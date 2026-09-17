//! # `state.rs` — Shared application state, transfer queue, and progress
//! aggregation (PRD §6.1, §6.4, FR-5.2, FR-5.3)
//!
//! Everything the backend owns lives in [`AppState`], shared as an `Arc`
//! between the Tauri commands and the two background loops. Each field carries
//! its own lock rather than the whole struct sitting behind one, so the 3 s
//! heartbeat reading the alias never has to wait on a transfer updating its
//! progress.
//!
//! [`ProgressTracker`] is the other half of the module: FR-5.2's honest
//! telemetry, computed from bytes actually committed to disk.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex, Notify, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::error::TransferError;
use crate::history::{Direction, HistoryEntry};
use crate::registry::PeerRegistry;
use crate::settings::{Identity, Settings, Store};

/// FR-2.6 — at most three peers are streamed to concurrently.
const MAX_CONCURRENT_PEERS: usize = 3;
/// FR-5.2 — progress is emitted at ~10 Hz regardless of chunk cadence.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// FR-5.2 — instantaneous rate is a rolling one-second average.
const RATE_WINDOW: Duration = Duration::from_secs(1);

pub mod events {
    pub const PEER_DISCOVERED: &str = "peer://discovered";
    pub const PEER_UPDATED: &str = "peer://updated";
    pub const PEER_LOST: &str = "peer://lost";
    /// FR-1.5 — announced once per device, rendered as a dismissible chip.
    pub const PEER_INCOMPATIBLE: &str = "peer://incompatible";
    pub const TRANSFER_OFFER: &str = "transfer://offer";
    pub const TRANSFER_PROGRESS: &str = "transfer://progress";
    pub const TRANSFER_COMPLETE: &str = "transfer://complete";
    pub const TRANSFER_ERROR: &str = "transfer://error";
    pub const NETWORK_CHANGED: &str = "network://changed";
    pub const SETTINGS_CHANGED: &str = "settings://changed";
}

/// Lifecycle of a transfer (§6.4). Both ends track this independently and
/// label it identically, so the two screens agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferPhase {
    Queued,
    Offered,
    Transferring,
    Completed,
    Cancelled,
    Failed,
    Rejected,
}

/// §6.4 — the snapshot both ends render identically.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSnapshot {
    pub id: String,
    pub direction: Direction,
    pub peer_alias: String,
    pub peer_device_id: String,
    pub phase: TransferPhase,
    pub label: String,
    pub current_file: String,
    pub index: usize,
    pub count: usize,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub rate_bps: u64,
    pub eta_seconds: Option<u64>,
    pub is_text: bool,
}

/// An incoming offer awaiting the user's decision (FR-4.1).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferPayload {
    pub id: String,
    pub peer_alias: String,
    pub peer_device_id: String,
    pub item_count: usize,
    pub total_bytes: u64,
    pub is_text: bool,
    pub preview: Vec<String>,
}

/// A failure, shaped for the frontend toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferErrorPayload {
    pub id: String,
    pub direction: Direction,
    pub peer_alias: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
}

/// FR-5.2 — tracks bytes *committed to disk*, never bytes handed to the socket
/// buffer. On the sending side that means bytes the peer has accepted into its
/// write; on the receiving side, bytes we have written.
pub struct ProgressTracker {
    snapshot: TransferSnapshot,
    window: VecDeque<(Instant, u64)>,
    last_emit: Instant,
    started: Instant,
}

impl ProgressTracker {
    pub fn new(snapshot: TransferSnapshot) -> Self {
        let now = Instant::now();
        Self {
            snapshot,
            // Seeded with a zero sample so `recompute_rate` always has a window
            // front to measure from, even before the first chunk lands.
            window: VecDeque::from([(now, 0)]),
            // Backdated by one interval so the very first `emit` is not
            // throttled — the UI needs the queued row immediately.
            last_emit: now - PROGRESS_INTERVAL,
            started: now,
        }
    }

    pub fn snapshot(&self) -> TransferSnapshot {
        self.snapshot.clone()
    }

    pub fn set_phase(&mut self, phase: TransferPhase) {
        self.snapshot.phase = phase;
    }

    pub fn set_label(&mut self, label: String) {
        self.snapshot.label = label;
    }

    pub fn set_count(&mut self, count: usize) {
        self.snapshot.count = count;
    }

    /// Adjusts the denominator when an entry is skipped mid-transfer, so the
    /// bar still reaches 100% (UI-3 — progress never goes backward).
    ///
    /// Clamped to at least `bytes_done`: a total below the bytes already
    /// written would put the bar past 100% and, worse, make the remaining-byte
    /// subtraction in `recompute_rate` underflow.
    pub fn set_total(&mut self, total: u64) {
        self.snapshot.total_bytes = total.max(self.snapshot.bytes_done);
    }

    pub fn total(&self) -> u64 {
        self.snapshot.total_bytes
    }

    pub fn done(&self) -> u64 {
        self.snapshot.bytes_done
    }

    pub fn begin_item(&mut self, name: &str, index: usize) {
        self.snapshot.current_file = name.to_string();
        self.snapshot.index = index;
    }

    /// Record `n` bytes committed to disk, and refresh the derived rate.
    ///
    /// Called once per 128 KB chunk, so this is the hottest function in the
    /// module — hence a `VecDeque` of cumulative samples rather than anything
    /// that would reallocate or re-sum the history.
    pub fn add_bytes(&mut self, n: u64) {
        self.snapshot.bytes_done += n;
        let now = Instant::now();
        self.window.push_back((now, self.snapshot.bytes_done));

        // Evict samples older than the one-second window (FR-5.2). A `while`
        // rather than an `if` because a stalled link can leave several stale
        // samples to drop at once when traffic resumes.
        //
        // `len() > 2` is the important guard: it keeps at least two samples so
        // there is always an interval to divide by. Without it, a slow transfer
        // whose only samples are older than a second would empty the window and
        // the rate would read zero while bytes were still moving.
        while let Some(&(t, _)) = self.window.front() {
            if now.duration_since(t) > RATE_WINDOW && self.window.len() > 2 {
                self.window.pop_front();
            } else {
                // Front is inside the window; everything behind it is newer.
                break;
            }
        }
        self.recompute_rate(now);
    }

    /// Derive rate and ETA from the rolling window.
    fn recompute_rate(&mut self, now: Instant) {
        // The window is never empty in practice — `new` seeds it and
        // `add_bytes` keeps two samples — but the fallback avoids an `unwrap`
        // in the hot path rather than relying on that invariant.
        let (first_t, first_b) = *self.window.front().unwrap_or(&(self.started, 0));
        let elapsed = now.duration_since(first_t).as_secs_f64();

        // Below 50 ms the divisor is too small to be meaningful: the figure
        // would swing wildly between chunks and the displayed rate would be
        // unreadable. Keeping the previous value is what UI-2's stable numerals
        // require, and it converges within a couple of chunks.
        let rate = if elapsed > 0.05 {
            ((self.snapshot.bytes_done - first_b) as f64 / elapsed) as u64
        } else {
            self.snapshot.rate_bps
        };
        self.snapshot.rate_bps = rate;

        // ETA is only meaningful with a nonzero rate and bytes still to send.
        // `None` is the honest answer otherwise — the frontend renders it as
        // "—" rather than inventing a number. Both conditions are needed: a
        // zero rate would divide by zero, and a completed transfer would
        // underflow the subtraction.
        self.snapshot.eta_seconds =
            if rate > 0 && self.snapshot.total_bytes > self.snapshot.bytes_done {
                // `max(1)` is belt-and-braces against the `rate > 0` test above.
                Some((self.snapshot.total_bytes - self.snapshot.bytes_done) / rate.max(1))
            } else {
                None
            };
    }

    /// Emit at most every `PROGRESS_INTERVAL`; `force` bypasses the throttle for
    /// phase transitions, which must never be dropped.
    ///
    /// This is FR-5.2's "decoupled from chunk cadence": on a gigabit link
    /// chunks land far faster than 10 Hz, and emitting per chunk would flood
    /// the IPC channel and make the webview the bottleneck. The `force` escape
    /// exists because a dropped phase change would leave a row stuck showing
    /// "Transferring" forever.
    pub fn emit(&mut self, app: &AppHandle, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_emit) < PROGRESS_INTERVAL {
            return;
        }
        // Stamped only when an emit actually happens, so the throttle measures
        // the gap between deliveries rather than between attempts.
        self.last_emit = now;
        let _ = app.emit(events::TRANSFER_PROGRESS, self.snapshot.clone());
    }
}

struct ActiveTransfer {
    cancel: CancellationToken,
    snapshot: TransferSnapshot,
}

/// Everything the backend owns: settings, peers, the transfer queue, history.
///
/// Shared as `Arc<AppState>` between the Tauri commands and the background
/// discovery and transfer loops.
pub struct AppState {
    pub store: Store,
    pub identity: RwLock<Identity>,
    pub settings: RwLock<Settings>,
    pub registry: Mutex<PeerRegistry>,
    history: Mutex<Vec<HistoryEntry>>,

    transfers: Mutex<HashMap<String, ActiveTransfer>>,
    /// FR-4.1 — offers awaiting a user decision, keyed by transfer id.
    offers: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// FR-2.6 — one lock per peer makes sends to that peer sequential.
    peer_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    outbound_slots: Arc<Semaphore>,

    active_count: Arc<AtomicUsize>,
    /// Actual bound ports after fallback (§6.2); may differ from settings.
    pub bound_ports: RwLock<(u16, u16)>,
    /// FR-1.6 — pulsed by `rescan` to trigger an immediate heartbeat.
    pub rescan: Notify,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        let settings = store.load_settings();
        let identity = store.load_identity();
        let history = store.load_history();
        let ports = (settings.discovery_port, settings.transfer_port);

        Self {
            store,
            identity: RwLock::new(identity),
            settings: RwLock::new(settings),
            registry: Mutex::new(PeerRegistry::new()),
            history: Mutex::new(history),
            transfers: Mutex::new(HashMap::new()),
            offers: Mutex::new(HashMap::new()),
            peer_locks: Mutex::new(HashMap::new()),
            outbound_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_PEERS)),
            active_count: Arc::new(AtomicUsize::new(0)),
            bound_ports: RwLock::new(ports),
            rescan: Notify::new(),
        }
    }

    /// Track a new transfer and return the token that cancels it.
    pub async fn register(&self, snapshot: TransferSnapshot) -> CancellationToken {
        let cancel = CancellationToken::new();
        self.transfers.lock().await.insert(
            snapshot.id.clone(),
            ActiveTransfer {
                cancel: cancel.clone(),
                snapshot,
            },
        );
        cancel
    }

    pub async fn update(&self, snapshot: TransferSnapshot) {
        // Silently ignores an unknown id, which is correct rather than lax: a
        // final update can race the `finish` that removed the entry, and
        // re-inserting it would resurrect a transfer that has already ended.
        if let Some(entry) = self.transfers.lock().await.get_mut(&snapshot.id) {
            entry.snapshot = snapshot;
        }
    }

    pub async fn finish(&self, id: &str) {
        self.transfers.lock().await.remove(id);
    }

    pub async fn active_transfers(&self) -> Vec<TransferSnapshot> {
        self.transfers
            .lock()
            .await
            .values()
            .map(|t| t.snapshot.clone())
            .collect()
    }

    /// FR-2.7 — cancellation is cooperative; the streaming loops poll the token
    /// between chunks and unwind, closing the socket and deleting `.part`.
    pub async fn cancel(&self, id: &str) -> bool {
        // A transfer still awaiting a decision is rejected rather than aborted.
        // Done first and unconditionally: an offer and an active transfer are
        // different stages of the same id, and cancelling during the prompt has
        // to answer the waiting `oneshot` or the sender would sit until its
        // 30 s timeout instead of learning immediately.
        if let Some(tx) = self.offers.lock().await.remove(id) {
            // Send failing means the receiver is already gone (the timeout
            // fired), which is not an error — the outcome is the same.
            let _ = tx.send(false);
        }

        // The return value distinguishes "cancelled something" from "no such
        // transfer", which the command passes back so the UI does not report
        // success for an id that had already finished.
        match self.transfers.lock().await.get(id) {
            Some(t) => {
                // Cooperative: this only sets the token. The streaming loops
                // notice between chunks and unwind, which is what actually
                // closes the socket and deletes the `.part` file.
                t.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancels every in-flight transfer so `.part` files are cleaned up rather
    /// than orphaned (§6.6).
    ///
    /// Desktop-only: the caller is the tray "Quit" action. Android has no
    /// equivalent — the OS terminates the process, and orphaned `.part` files
    /// are swept on next launch by `transfer::cleanup_orphans`.
    #[cfg(desktop)]
    pub async fn cancel_all(&self) {
        // Signals every token; the entries are not removed, because each
        // transfer's own task does that as it unwinds. Fast enough to run
        // synchronously on the quit path — setting a token does no I/O.
        for t in self.transfers.lock().await.values() {
            t.cancel.cancel();
        }
    }

    /// Hold an offer until the user answers, or the caller times out (FR-4.2).
    ///
    /// A `oneshot` because the answer arrives exactly once, from one of three
    /// places: the accept dialog, a cancellation, or the 30 s timeout. The
    /// receiver is handed to the connection task, which awaits it before
    /// sending the ACK byte.
    pub async fn park_offer(&self, id: String) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.offers.lock().await.insert(id, tx);
        rx
    }

    /// Deliver the user's decision. Returns false if the offer already lapsed.
    ///
    /// Two ways to fail, both reported as `false`: no such offer (already
    /// answered or timed out), or the receiver having been dropped. The command
    /// layer passes this back so the UI does not claim to have answered an
    /// offer the sender has already given up on.
    pub async fn answer_offer(&self, id: &str, accept: bool) -> bool {
        match self.offers.lock().await.remove(id) {
            Some(tx) => tx.send(accept).is_ok(),
            None => false,
        }
    }

    pub async fn drop_offer(&self, id: &str) {
        self.offers.lock().await.remove(id);
    }

    /// FR-2.6 — serialize per peer, cap concurrency across peers.
    ///
    /// A lock per device id, created on first use. Two mechanisms working
    /// together: this mutex makes sends to one peer strictly sequential (so a
    /// second batch queues behind the first rather than interleaving), while
    /// `outbound_slots` caps how many *different* peers can be streamed to at
    /// once. Neither alone would satisfy FR-2.6.
    ///
    /// The map is never pruned. Bounded by the number of distinct peers ever
    /// seen in one session, each entry a pointer-sized mutex — not worth the
    /// bookkeeping a cleanup would need to avoid dropping a lock still held.
    pub async fn peer_lock(&self, device_id: &str) -> Arc<Mutex<()>> {
        self.peer_locks
            .lock()
            .await
            .entry(device_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            // Cloned out so the outer `peer_locks` lock is released before the
            // caller waits on the inner one — holding both would serialize
            // every peer against every other, defeating the point.
            .clone()
    }

    pub fn outbound_slots(&self) -> Arc<Semaphore> {
        self.outbound_slots.clone()
    }

    /// Drives the `busy` flag in our heartbeat (§6.2), the Android keep-awake
    /// hint, and — once wired — the foreground service (AND-2).
    ///
    /// The screen hint is toggled only on the 0↔1 edges, so overlapping
    /// transfers do not fight over it.
    pub fn enter_active(&self) -> ActiveGuard {
        // `fetch_add` returns the *previous* value, so `== 0` identifies the
        // transition from idle to active — the single moment the native hints
        // should be turned on. Without the edge check, three concurrent
        // transfers would each re-request them, and the first to finish would
        // release what the other two still need.
        if self.active_count.fetch_add(1, Ordering::SeqCst) == 0 {
            crate::android::set_keep_screen_on(true);
            // Promotes the ongoing notification from "listening" to "transferring".
            // Starting the service here too covers the case where background
            // mode is off: the transfer still needs the process kept alive.
            crate::android::start_background_service(Some("Transferring files…"));
        }
        ActiveGuard {
            counter: self.active_count.clone(),
        }
    }

    /// Whether any transfer is in flight; drives the `busy` heartbeat flag.
    pub fn is_busy(&self) -> bool {
        self.active_count.load(Ordering::SeqCst) > 0
    }

    pub async fn push_history(&self, entry: HistoryEntry) {
        // Persisted while the lock is still held, so the file and the in-memory
        // list cannot diverge: two transfers finishing at once would otherwise
        // both write, and the loser's entry would be missing from disk.
        let mut history = self.history.lock().await;
        crate::history::push(&mut history, entry);
        self.store.save_history(&history);
    }

    pub async fn history(&self) -> Vec<HistoryEntry> {
        self.history.lock().await.clone()
    }

    pub async fn clear_history(&self) {
        let mut history = self.history.lock().await;
        history.clear();
        self.store.save_history(&history);
    }

    /// FR-5.6 — report a failure to the user and the log in one place.
    ///
    /// The split is the point: `detail` (a raw OS or serde string) goes only to
    /// the log, while the frontend receives the code and the human sentence.
    /// Routing every failure through here is what keeps a raw Rust error from
    /// ever reaching the UI.
    pub async fn emit_error(
        &self,
        app: &AppHandle,
        id: &str,
        direction: Direction,
        alias: &str,
        err: &TransferError,
    ) {
        let payload = err.payload(alias);
        tracing::warn!(id, code = payload.code, detail = ?payload.detail, "transfer failed");
        let _ = app.emit(
            events::TRANSFER_ERROR,
            TransferErrorPayload {
                id: id.to_string(),
                direction,
                peer_alias: alias.to_string(),
                code: payload.code,
                message: payload.message,
                detail: payload.detail,
            },
        );
    }
}

/// Held for the duration of one transfer; releases the native hints on drop.
///
/// RAII rather than a matching `exit_active()` call, because a transfer can end
/// at a dozen different points — completion, cancellation, a read error, a
/// dropped connection — and every one of them must release. A paired call would
/// have to be repeated at each exit, and the one that got missed would leave
/// the screen awake indefinitely.
pub struct ActiveGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        // Release only when the last transfer finishes. Running in Drop means
        // this happens on cancellation and on error too, not just on the happy
        // path — a leaked wake lock would quietly drain the battery.
        //
        // `fetch_sub` returns the previous count, so `== 1` is the active-to-idle
        // edge — the mirror of the `== 0` test in `enter_active`.
        if self.counter.fetch_sub(1, Ordering::SeqCst) == 1 {
            crate::android::set_keep_screen_on(false);
            // Back to the idle notification; the service itself keeps running so
            // the device stays discoverable.
            crate::android::start_background_service(None);
        }
    }
}
