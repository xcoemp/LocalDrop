//! # `transfer.rs` — TCP transfer engine, send and receive (PRD §6.3, FR-2,
//! FR-3, FR-4)
//!
//! The largest module in the backend, and the one where the protocol actually
//! happens. Two halves mirror each other: [`run_listener`] accepts inbound
//! connections and [`spawn_send`] starts outbound ones, and both drive the same
//! §6.4 state machine through a [`ProgressTracker`].
//!
//! Three invariants hold throughout, and most of the comments below exist to
//! explain how a given piece of control flow preserves one of them:
//!
//! 1. **Never buffer a whole file** (FR-2.5). Everything streams in 128 KB
//!    chunks, so memory use is flat regardless of payload size.
//! 2. **Never trust the peer** (SEC-1…SEC-5). Paths are sanitized and
//!    bounds-checked before the ACK, declared sizes are enforced byte-for-byte,
//!    and a malformed frame closes the connection without a partial write.
//! 3. **Never leave a partial file looking complete** (FR-2.4). Data lands in
//!    `.part` and is renamed only on success; cancellation and failure both
//!    delete it.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>
//!
//! # Protocol addendum: per-entry prelude byte
//!
//! §6.3 as written has no per-entry framing, which makes two other
//! requirements unsatisfiable: FR-2.3 (skip symlinks) and §6.6 ("source file
//! deleted mid-transfer → fail that entry, continue the remaining manifest").
//! With fixed sizes and no framing, a sender that cannot produce a declared
//! entry has no way to say so — it must either pad with garbage (silently
//! corrupting the file) or drop the connection (losing the rest of the batch).
//!
//! So each non-directory manifest entry is preceded by one byte:
//!   `0x01` ENTRY_DATA — exactly `size` bytes follow
//!   `0x00` ENTRY_SKIP — no bytes follow; the receiver moves to the next entry
//!
//! This keeps the stream self-synchronizing at entry boundaries. PRD §6.3
//! should be updated to match.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_lite::stream::StreamExt;
use tauri::{AppHandle, Emitter};
use tauri_plugin_fs::FsExt;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{TransferError, TransferResult};
use crate::history::{now_ms, Direction, HistoryEntry, Outcome};
use crate::paths;
use crate::protocol::*;
use crate::registry::Peer;
use crate::settings::Settings;
use crate::state::{
    events, AppState, OfferPayload, ProgressTracker, TransferPhase, TransferSnapshot,
};

const ENTRY_DATA: u8 = 0x01;
const ENTRY_SKIP: u8 = 0x00;

/// Socket buffer wrapped around the raw stream. 128 KB matches CHUNK_SIZE so a
/// chunk write becomes one syscall.
const SOCKET_BUF: usize = CHUNK_SIZE;

/// Accept inbound transfers forever, rebinding if the socket dies.
///
/// Each connection is handled on its own task, so a slow or malicious peer
/// cannot block the others.
pub async fn run_listener(app: AppHandle, state: Arc<AppState>) {
    // Two nested loops with distinct jobs: the outer one owns a binding, the
    // inner one accepts on it. Separating them is what makes a dead socket
    // recoverable — `break` from the inner loop drops the listener and the
    // outer loop binds a fresh one.
    loop {
        let port = state.settings.read().await.transfer_port;

        let listener = match bind(port).await {
            Ok(l) => l,
            Err(err) => {
                // Same policy as discovery: report and retry rather than give
                // up, since the port may be freed by another program shortly.
                state
                    .emit_error(&app, "listener", Direction::Incoming, "LocalDrop", &err)
                    .await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        // §6.2 — recorded so the heartbeat advertises the port actually bound.
        // A sender reads `tcp_port` from the packet, so fallback is transparent.
        let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        state.bound_ports.write().await.1 = bound;
        tracing::info!(port = bound, "transfer listener bound");

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    // Spawned per connection, which is what §6.6's "two peers
                    // sending at once" case requires: the accept loop returns
                    // immediately and both transfers proceed independently.
                    // A slow or malicious peer therefore cannot block the rest.
                    let app = app.clone();
                    let state = state.clone();
                    tokio::spawn(async move {
                        // Errors are logged at debug, not surfaced: a peer
                        // disconnecting mid-handshake is routine, and anything
                        // the *user* needs to know was already emitted from
                        // inside `handle_incoming` with proper context.
                        if let Err(e) = handle_incoming(app, state, stream, addr).await {
                            tracing::debug!(error = ?e, "incoming connection ended");
                        }
                    });
                }
                Err(e) => {
                    // An accept error is the socket itself failing — typically
                    // the interface going down — not one bad connection, so the
                    // whole binding is abandoned rather than retried in place.
                    tracing::warn!(error = %e, "accept failed; rebinding");
                    break;
                }
            }
        }

        // Brief pause before rebinding, so a persistently failing socket cannot
        // spin this loop at full speed.
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// §6.2 — port fallback, the TCP counterpart of `discovery::bind`.
///
/// Simpler than the UDP version because there is no broadcast option to set:
/// the first port that binds is the answer.
async fn bind(port: u16) -> TransferResult<TcpListener> {
    for candidate in port..=port.saturating_add(PORT_FALLBACK_RANGE) {
        if let Ok(listener) =
            TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, candidate))).await
        {
            return Ok(listener);
        }
    }
    Err(TransferError::PortUnavailable {
        first: port,
        last: port.saturating_add(PORT_FALLBACK_RANGE),
    })
}

/// Drive one inbound connection through §6.3's handshake and receive.
///
/// The receiving half of the protocol, in order: read the offer, validate it,
/// plan every path, check free space, ask the user, ACK, stream, exchange
/// trailers. Every step that can refuse does so before the ACK byte is written,
/// so nothing touches the disk until the transfer is certain to proceed.
async fn handle_incoming(
    app: AppHandle,
    state: Arc<AppState>,
    stream: TcpStream,
    addr: SocketAddr,
) -> TransferResult<()> {
    // Nagle off: the control plane is a sequence of small frames each awaiting
    // a reply, and coalescing them would add up to 200 ms per handshake step.
    let _ = stream.set_nodelay(true);

    // Split so the read and write halves can be buffered independently — the
    // handshake interleaves them, and a single buffered stream would need the
    // borrow released between every step.
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::with_capacity(SOCKET_BUF, read_half);
    let mut writer = BufWriter::with_capacity(SOCKET_BUF, write_half);

    // §6.3 step 2 — the offer header is the first frame on every connection.
    // Validated immediately, before anything else is read or allocated: this is
    // SEC-2 and SEC-5's fail-closed boundary, and a `?` here closes the
    // connection with nothing written to disk.
    let offer: OfferHeader = read_frame(&mut reader).await?;
    validate_offer(&offer)?;

    // Locally generated, not taken from the peer: a sender-chosen id could
    // collide with or impersonate another transfer's.
    let id = Uuid::new_v4().to_string();
    // The alias is remote display data — see `sanitize_alias`.
    let alias = sanitize_alias(&offer.sender_alias, addr);
    // Snapshotted once for the whole transfer, so a setting changed mid-receive
    // cannot switch the collision policy or download directory halfway through.
    let settings = state.settings.read().await.clone();

    tracing::info!(
        %addr, alias, items = offer.item_count, bytes = offer.total_size, "offer received"
    );

    // SEC-1 — every path is sanitized and bounds-checked before we decide to
    // accept, so a malicious manifest is rejected before any disk work.
    //
    // The whole manifest is planned up front rather than per entry as it
    // arrives. That ordering is the point: a batch containing one traversal
    // attempt is refused in its entirety, and the user is never prompted to
    // accept a transfer that was always going to be rejected. Both gates run
    // here — the rebuild in `sanitize_rel_path`, then `ensure_within` on the
    // joined result.
    let root = settings.download_path();
    let mut planned: Vec<(ManifestEntry, PathBuf)> = Vec::with_capacity(offer.manifest.len());
    for entry in &offer.manifest {
        let safe = paths::sanitize_rel_path(&entry.rel_path)?;
        let target = root.join(&safe);
        paths::ensure_within(&root, &target)?;
        planned.push((entry.clone(), safe));
    }

    // FR-2.9 — free space is checked before the ACK, not after.
    //
    // `if let` rather than a default, so an unknown capacity (an unusual
    // filesystem, or `sysinfo` failing to enumerate the disk) *permits* the
    // transfer rather than blocking it. Refusing on a failed check would be a
    // worse failure than running out of space, which the write itself reports.
    if let Some(available) = available_space(&root) {
        if offer.total_size > available {
            let err = TransferError::InsufficientSpace {
                needed: offer.total_size,
                available,
            };
            reject(&mut writer, &err).await?;
            state
                .emit_error(&app, &id, Direction::Incoming, &alias, &err)
                .await;
            return Ok(());
        }
    }

    let is_text = offer.payload_type == PayloadType::Text;
    let label = describe(&offer);

    // FR-4.1 / FR-4.3 — auto-accept skips the prompt but stays visible in the UI.
    //
    // The whole consent decision resolves to one boolean before any data is
    // read, so there is exactly one place below that writes the ACK byte.
    let accepted = if settings.auto_accept {
        true
    } else {
        // Parked *before* the event is emitted: the frontend can answer within
        // a frame, and registering afterwards would race a fast accept.
        let rx = state.park_offer(id.clone()).await;
        let _ = app.emit(
            events::TRANSFER_OFFER,
            OfferPayload {
                id: id.clone(),
                peer_alias: alias.clone(),
                peer_device_id: offer.sender_device_id.clone(),
                item_count: offer.item_count,
                total_bytes: offer.total_size,
                is_text,
                // Directories are filtered out (they are structure, not
                // content) and the list is capped at five: SEC-2 permits
                // 100,000 entries, and the dialog only has room to reassure the
                // user about what is arriving. The count beside it is exact.
                preview: offer
                    .manifest
                    .iter()
                    .filter(|e| !e.is_dir)
                    .take(5)
                    .map(|e| e.rel_path.clone())
                    .collect(),
            },
        );

        // FR-4.2 — no answer in 30 s is a rejection, so the sender is never
        // blocked on an unattended device.
        //
        // Three outcomes collapse into two arms. Only an explicit answer is
        // honoured; both the timeout and a dropped sender (`Err`) mean the same
        // thing — nobody said yes — and the wildcard treats them alike. The
        // `drop_offer` then removes the parked channel so a late click cannot
        // answer an offer whose connection has already moved on.
        match tokio::time::timeout(OFFER_TIMEOUT, rx).await {
            Ok(Ok(answer)) => answer,
            _ => {
                state.drop_offer(&id).await;
                false
            }
        }
    };

    // Declined, or nobody answered. The sender is told first, then the refusal
    // is recorded locally — FR-5.4's history is a log of everything that
    // happened, not only of what succeeded.
    if !accepted {
        // Empty reason: the sender substitutes its own generic wording. A
        // decline carries no explanation on purpose — the receiving user is not
        // obliged to justify it to the other device.
        reject(&mut writer, &TransferError::Rejected(String::new())).await?;
        state
            .push_history(HistoryEntry {
                id: id.clone(),
                direction: Direction::Incoming,
                peer_alias: alias.clone(),
                peer_device_id: offer.sender_device_id.clone(),
                label,
                item_count: offer.item_count,
                total_bytes: offer.total_size,
                outcome: Outcome::Rejected,
                finished_at: now_ms(),
                path: None,
                text: None,
                error_code: None,
                error_message: None,
                skipped: Vec::new(),
            })
            .await;
        // Emits the bare id, not a history entry — the frontend's
        // `transfer://complete` handler discriminates on the payload type and
        // uses a string to mean "this offer is gone, close its dialog".
        let _ = app.emit(events::TRANSFER_COMPLETE, id);
        return Ok(());
    }

    // §6.3 step 3 — the single ACCEPT byte. Explicitly flushed: the writer is
    // buffered, and a one-byte write would otherwise sit in the buffer while
    // the sender waited for permission to start.
    writer
        .write_all(&[ACK_ACCEPT])
        .await
        .map_err(TransferError::connection)?;
    writer.flush().await.map_err(TransferError::connection)?;

    // RAII guard: marks this device `busy` in its heartbeat (§6.2) and holds
    // the Android keep-awake hint until it drops at the end of this function —
    // including on every `?` early return below. Bound to a named `_active`
    // rather than `_`, which would drop it immediately.
    let _active = state.enter_active();

    let snapshot = TransferSnapshot {
        id: id.clone(),
        direction: Direction::Incoming,
        peer_alias: alias.clone(),
        peer_device_id: offer.sender_device_id.clone(),
        phase: TransferPhase::Transferring,
        label: label.clone(),
        current_file: String::new(),
        index: 0,
        count: offer.item_count,
        bytes_done: 0,
        total_bytes: offer.total_size,
        rate_bps: 0,
        eta_seconds: None,
        is_text,
    };
    let cancel = state.register(snapshot.clone()).await;
    let mut progress = ProgressTracker::new(snapshot);
    // Forced, so the row appears immediately rather than after the first
    // throttle window — the user pressed Accept and expects to see something.
    progress.emit(&app, true);

    // The two payload shapes diverge here and nowhere else. `receive_text`
    // reads one body of `total_size` bytes; `receive_files` walks the manifest
    // with its prelude bytes. Both return the same outcome type so
    // `finish_receive` below is shared.
    let outcome = if is_text {
        receive_text(&app, &mut reader, &mut progress, &offer, &cancel).await
    } else {
        receive_files(
            &app,
            &mut reader,
            &mut progress,
            &planned,
            &settings,
            &alias,
            &cancel,
        )
        .await
    };

    finish_receive(
        &app,
        &state,
        &id,
        &alias,
        &offer,
        &mut progress,
        outcome,
        &mut reader,
        &mut writer,
    )
    .await;
    Ok(())
}

type ReceiveOutcome = TransferResult<(Option<PathBuf>, Option<String>, Vec<String>)>;

async fn receive_text<R>(
    app: &AppHandle,
    reader: &mut R,
    progress: &mut ProgressTracker,
    offer: &OfferHeader,
    cancel: &CancellationToken,
) -> ReceiveOutcome
where
    R: AsyncReadExt + Unpin,
{
    progress.begin_item("Text snippet", 1);

    // The one place a whole payload is buffered in memory, and it is bounded:
    // `validate_offer` has already rejected a text offer above `MAX_TEXT_BYTES`,
    // and `min` here means even a lying `total_size` cannot make this allocate
    // more than 1 MB. A snippet has to be complete to be UTF-8-validated, so
    // FR-2.5's no-buffering rule does not apply to it.
    let mut buf = Vec::with_capacity(offer.total_size.min(MAX_TEXT_BYTES) as usize);
    let mut remaining = offer.total_size;
    let mut chunk = vec![0u8; CHUNK_SIZE.min(MAX_TEXT_BYTES as usize)];

    // Driven by a remaining-byte counter, not by end-of-stream: the connection
    // stays open afterwards for the completion trailer, so reading to EOF would
    // block forever.
    while remaining > 0 {
        // FR-2.7 — checked once per chunk. This is the cooperative half of
        // cancellation: `cancel` only sets a flag, and this is where it is seen.
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }

        // SEC-3 — never read more than what remains declared, so a sender
        // cannot bleed extra bytes into the trailer that follows.
        let want = remaining.min(chunk.len() as u64) as usize;
        let n = reader
            .read(&mut chunk[..want])
            .await
            .map_err(TransferError::connection)?;

        // A zero-length read means EOF with bytes still owed — the peer went
        // away mid-snippet. Distinguished from an error because the read itself
        // succeeded, and it must not loop forever on a closed socket.
        if n == 0 {
            return Err(TransferError::ConnectionLost("stream ended early".into()));
        }

        buf.extend_from_slice(&chunk[..n]);
        remaining -= n as u64;
        progress.add_bytes(n as u64);
        // Unforced: the 10 Hz throttle applies, since a snippet can arrive in a
        // handful of chunks and every one of them would otherwise emit.
        progress.emit(app, false);
    }

    // SEC-5 — invalid UTF-8 is a protocol violation, not something to render
    // lossily: the sender declared this a text payload.
    let text = String::from_utf8(buf).map_err(|_| TransferError::MalformedPayload)?;
    Ok((None, Some(text), Vec::new()))
}

async fn receive_files<R>(
    app: &AppHandle,
    reader: &mut R,
    progress: &mut ProgressTracker,
    planned: &[(ManifestEntry, PathBuf)],
    settings: &Settings,
    sender_alias: &str,
    cancel: &CancellationToken,
) -> ReceiveOutcome
where
    R: AsyncReadExt + Unpin,
{
    let root = settings.download_path();
    // Recorded so history can offer "Show in Folder" on a single-file receive.
    let mut first_file: Option<PathBuf> = None;
    let mut skipped: Vec<String> = Vec::new();
    // Any `.part` still open when we unwind must be deleted (FR-2.7).
    let mut in_flight: Option<PathBuf> = None;

    // The whole walk runs inside an async block whose result is captured rather
    // than propagated with `?`. That is what guarantees the cleanup below runs
    // on every path — success, cancellation, a write error, a malformed
    // prelude. Using `?` directly in this function would skip it and leave a
    // `.part` file behind, which FR-2.4 and §6.6 both forbid.
    let result = async {
        for (index, (entry, rel)) in planned.iter().enumerate() {
            // Checked per entry as well as per chunk, so cancelling during a
            // batch of thousands of tiny files takes effect promptly (S5).
            if cancel.is_cancelled() {
                return Err(TransferError::Cancelled);
            }

            // FR-4.4 — organize-by-sender / by-type applies to the leading
            // directory only, so a folder send stays internally intact.
            let base = paths::organize_dir(
                &root,
                sender_alias,
                rel.file_name().map(Path::new).unwrap_or(rel.as_path()),
                settings.organize_by_sender,
                settings.organize_by_file_type && !entry.is_dir,
            );
            let target = base.join(rel);
            // SEC-1, third time: re-checked because `organize_dir` has just
            // rebuilt the base from the sender's alias, which is itself remote
            // input. The earlier check covered `root.join(rel)`, not this path.
            paths::ensure_within(&root, &target)?;

            // Directory entries carry no bytes and therefore no prelude —
            // handled entirely before the read below. Getting this wrong would
            // desynchronise the stream by one byte per directory.
            if entry.is_dir {
                // FR-2.2 — empty directories are recreated.
                tokio::fs::create_dir_all(paths::extended(&target))
                    .await
                    .map_err(|e| TransferError::write(&target, e))?;
                continue;
            }

            // The per-entry prelude byte described in the module docs.
            // `read_exact` rather than `read`: exactly one byte is owed, and a
            // short read here would misalign everything that follows.
            let mut prelude = [0u8; 1];
            reader
                .read_exact(&mut prelude)
                .await
                .map_err(TransferError::connection)?;

            // ENTRY_SKIP — the sender could not produce this entry (a symlink
            // per FR-2.3, or a file deleted mid-transfer per §6.6). No bytes
            // follow, so the loop simply advances.
            if prelude[0] == ENTRY_SKIP {
                skipped.push(entry.rel_path.clone());
                // The denominator shrinks so the bar still reaches 100%.
                // `saturating_sub` because a lying manifest could declare an
                // entry larger than the total it also declared.
                progress.set_total(progress.total().saturating_sub(entry.size));
                continue;
            }

            // Anything that is neither marker means the stream is no longer
            // where we think it is. There is no safe way to resynchronise, so
            // the connection dies rather than writing misaligned data.
            if prelude[0] != ENTRY_DATA {
                return Err(TransferError::MalformedPayload); // SEC-5
            }

            // Created lazily, per entry: FR-2.2's structure comes from the
            // manifest's paths, and a folder send's intermediate directories
            // may not have their own entries.
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(paths::extended(parent))
                    .await
                    .map_err(|e| TransferError::write(parent, e))?;
            }

            // FR-2.8 — collision policy decides the final name up front, but we
            // still stream into `.part` so a failure never clobbers a good file.
            let Some(final_path) = paths::resolve_collision(target, settings.collision_policy)
            else {
                // Skip policy: the bytes are still on the wire and must be
                // drained to keep the stream aligned.
                skipped.push(entry.rel_path.clone());
                drain(reader, entry.size, progress, app, cancel).await?;
                continue;
            };

            let part = paths::part_path(&final_path);
            // Registered *before* the write begins, so the cleanup below can
            // find it however this entry ends.
            in_flight = Some(part.clone());
            // `index + 1` because FR-5.1's readout is "3 of 7", one-based.
            progress.begin_item(&entry.rel_path, index + 1);
            // Forced: a filename change is a discrete event the user tracks,
            // and the throttle would drop it on a batch of small files.
            progress.emit(app, true);

            write_entry(app, reader, progress, &part, entry.size, cancel).await?;

            // FR-2.4 — the rename is the commit point. Only now does a
            // complete, verified-length file appear under its real name.
            tokio::fs::rename(paths::extended(&part), paths::extended(&final_path))
                .await
                .map_err(|e| TransferError::write(&final_path, e))?;
            // Cleared, so a later failure does not delete this finished file.
            in_flight = None;

            // First successfully written file only — history's `path` field
            // points at something openable, and for a multi-file batch there is
            // no single sensible answer.
            if first_file.is_none() {
                first_file = Some(final_path);
            }
        }
        Ok(())
    }
    .await;

    // Runs unconditionally, which is the reason for the async block above.
    // `in_flight` is `Some` only if the loop left an entry mid-write.
    if let Some(part) = in_flight {
        // FR-2.7 / §6.6 — partial files never survive a failure.
        let _ = tokio::fs::remove_file(paths::extended(&part)).await;
    }

    // Propagated only after cleanup. Reversing these two lines is precisely the
    // bug the async block exists to prevent.
    result?;
    Ok((first_file, None, skipped))
}

/// Stream exactly `size` bytes to `part`. SEC-3: never reads past the declared
/// length, so a lying sender cannot bleed into the next entry.
async fn write_entry<R>(
    app: &AppHandle,
    reader: &mut R,
    progress: &mut ProgressTracker,
    part: &Path,
    size: u64,
    cancel: &CancellationToken,
) -> TransferResult<()>
where
    R: AsyncReadExt + Unpin,
{
    let file = File::create(paths::extended(part))
        .await
        .map_err(|e| TransferError::write(part, e))?;
    let mut writer = BufWriter::with_capacity(CHUNK_SIZE, file);

    let mut remaining = size;
    // One chunk buffer, reused for every iteration — FR-2.5's "no more than one
    // chunk per active stream in memory". A 10 GB file and a 10 KB file consume
    // exactly the same 128 KB here, which is what makes §2.3's S4 achievable.
    let mut buf = vec![0u8; CHUNK_SIZE];

    while remaining > 0 {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }

        // SEC-3 — the read is capped at what remains *declared*, not at the
        // buffer size. This is the enforcement point: a sender that sends more
        // bytes than it declared cannot have them written, because the loop
        // stops asking, and the excess then fails the trailer parse instead.
        let want = remaining.min(CHUNK_SIZE as u64) as usize;
        let n = reader
            .read(&mut buf[..want])
            .await
            .map_err(TransferError::connection)?;
        if n == 0 {
            return Err(TransferError::ConnectionLost("stream ended early".into()));
        }

        // `write_all`, not `write`: a partial write would silently drop bytes
        // and leave the file shorter than its declared size.
        writer
            .write_all(&buf[..n])
            .await
            .map_err(|e| TransferError::write(part, e))?;
        remaining -= n as u64;

        // FR-5.2 — progress counts bytes committed to our disk.
        // Accounted after the write, never after the socket read: counting
        // received bytes would be the "optimistic buffer math" Principle 3
        // rules out.
        progress.add_bytes(n as u64);
        progress.emit(app, false);
    }

    writer
        .flush()
        .await
        .map_err(|e| TransferError::write(part, e))?;
    // §6.3 — fsync before the rename, so a crash cannot leave a renamed but
    // unwritten file.
    writer
        .into_inner()
        .sync_all()
        .await
        .map_err(|e| TransferError::write(part, e))?;
    Ok(())
}

/// Consume and discard `size` bytes (collision policy `Skip`).
///
/// The bytes are already in flight when the receiver decides to skip the entry,
/// and there is no way to tell the sender to stop mid-entry — the prelude byte
/// is the sender's signal, not the receiver's. So they must be read and thrown
/// away to keep the stream aligned for the next entry.
///
/// Structurally identical to `write_entry` minus the file: same cancellation
/// check, same SEC-3 cap, same progress accounting. Progress still advances,
/// because from the user's point of view those bytes did cross the network and
/// the bar should not stall on a skipped file.
async fn drain<R>(
    reader: &mut R,
    size: u64,
    progress: &mut ProgressTracker,
    app: &AppHandle,
    cancel: &CancellationToken,
) -> TransferResult<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut remaining = size;
    let mut buf = vec![0u8; CHUNK_SIZE];
    while remaining > 0 {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let want = remaining.min(CHUNK_SIZE as u64) as usize;
        let n = reader
            .read(&mut buf[..want])
            .await
            .map_err(TransferError::connection)?;
        if n == 0 {
            return Err(TransferError::ConnectionLost("stream ended early".into()));
        }
        // The read itself is the work; `buf` is overwritten next iteration.
        remaining -= n as u64;
        progress.add_bytes(n as u64);
        progress.emit(app, false);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn finish_receive<R, W>(
    app: &AppHandle,
    state: &Arc<AppState>,
    id: &str,
    alias: &str,
    offer: &OfferHeader,
    progress: &mut ProgressTracker,
    outcome: ReceiveOutcome,
    reader: &mut R,
    writer: &mut W,
) where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // Deregistered first, so the transfer stops appearing as active regardless
    // of which branch below runs.
    state.finish(id).await;

    // Selection on the outcome. Both arms do the same three things — set the
    // terminal phase, record history, notify the frontend — because §6.4
    // requires every transfer reach a labelled terminal state, not just the
    // successful ones.
    match outcome {
        Ok((path, text, skipped)) => {
            // §6.3 — trailer exchange closes the transfer cleanly.
            //
            // The trailer is read best-effort: the bytes are already on disk
            // and the rename has happened, so a peer that drops before sending
            // it has still delivered the files. Treating that as a failure
            // would discard a completed transfer.
            let trailer: TransferResult<CompletionTrailer> = read_frame(reader).await;
            let _ = writer.write_all(&[ACK_ACCEPT]).await;
            let _ = writer.flush().await;

            // Skips are merged from both ends: the receiver's own (collision
            // policy `Skip`) and the sender's (symlinks per FR-2.3, files
            // deleted mid-transfer per §6.6). Neither side knows the full list
            // alone, and FR-2.3's completion summary needs it.
            let remote_skipped = trailer.map(|t| t.skipped).unwrap_or_default();
            let mut all_skipped = skipped;
            all_skipped.extend(remote_skipped);
            // `dedup` only removes adjacent repeats, which is sufficient here:
            // the one overlap in practice is an entry both ends skipped, and
            // the two lists preserve manifest order.
            all_skipped.dedup();

            progress.set_phase(TransferPhase::Completed);
            // Forced — a terminal phase must never be dropped by the throttle,
            // or the UI would leave the row showing "Transferring" forever.
            progress.emit(app, true);

            let entry = HistoryEntry {
                id: id.to_string(),
                direction: Direction::Incoming,
                peer_alias: alias.to_string(),
                peer_device_id: offer.sender_device_id.clone(),
                label: describe(offer),
                item_count: offer.item_count,
                total_bytes: progress.total(),
                outcome: Outcome::Completed,
                finished_at: now_ms(),
                path: path.map(|p| p.to_string_lossy().into_owned()),
                text,
                error_code: None,
                error_message: None,
                skipped: all_skipped,
            };
            state.push_history(entry.clone()).await;
            let _ = app.emit(events::TRANSFER_COMPLETE, entry);
        }
        Err(err) => {
            // Cancellation is distinguished from failure throughout the UI:
            // the user chose one and not the other, so it is neither an error
            // to report nor coloured like one.
            let cancelled = matches!(err, TransferError::Cancelled);
            progress.set_phase(if cancelled {
                TransferPhase::Cancelled
            } else {
                TransferPhase::Failed
            });
            progress.emit(app, true);

            let payload = err.payload(alias);
            state
                .push_history(HistoryEntry {
                    id: id.to_string(),
                    direction: Direction::Incoming,
                    peer_alias: alias.to_string(),
                    peer_device_id: offer.sender_device_id.clone(),
                    label: describe(offer),
                    item_count: offer.item_count,
                    total_bytes: offer.total_size,
                    outcome: if cancelled {
                        Outcome::Cancelled
                    } else {
                        Outcome::Failed
                    },
                    finished_at: now_ms(),
                    path: None,
                    text: None,
                    error_code: Some(payload.code.clone()),
                    error_message: Some(payload.message.clone()),
                    skipped: Vec::new(),
                })
                .await;
            state
                .emit_error(app, id, Direction::Incoming, alias, &err)
                .await;
        }
    }
}

/// §6.3 step 3 — decline: a `0x00` byte, then a length-prefixed JSON reason.
///
/// Both parts matter. The byte is what unblocks the sender; the reason is what
/// lets it say "not enough free space on that device" rather than a bare
/// refusal. `"this device"` is the alias substituted into the message, because
/// from the sender's point of view the failure is about us.
async fn reject<W>(writer: &mut W, err: &TransferError) -> TransferResult<()>
where
    W: AsyncWriteExt + Unpin,
{
    writer
        .write_all(&[ACK_REJECT])
        .await
        .map_err(TransferError::connection)?;
    write_frame(
        writer,
        &RejectReason {
            code: err.code().to_string(),
            message: err.message("this device"),
        },
    )
    .await
}

/// What a send carries: picked items, or a text snippet.
pub enum Payload {
    /// Raw strings from the picker or an OS drop: absolute paths on desktop,
    /// `content://` URIs on Android.
    Files(Vec<String>),
    Text(String),
}

/// Entry point used by the `send_files` / `send_text` commands.
///
/// Returns the transfer id immediately; the transfer itself runs detached so
/// the UI is never blocked on the queue (FR-2.6).
pub async fn spawn_send(
    app: AppHandle,
    state: Arc<AppState>,
    peer: Peer,
    payload: Payload,
) -> String {
    let id = Uuid::new_v4().to_string();
    let is_text = matches!(payload, Payload::Text(_));

    let snapshot = TransferSnapshot {
        id: id.clone(),
        direction: Direction::Outgoing,
        peer_alias: peer.alias.clone(),
        peer_device_id: peer.device_id.clone(),
        // §6.4's initial state. The row appears as Queued immediately, before
        // the manifest is even built, so the user sees their action register.
        phase: TransferPhase::Queued,
        // A snippet's label is final; a file batch's is a placeholder until
        // `describe` can count the manifest. Walking a large folder takes long
        // enough that a blank label would look like a hang.
        label: String::from(if is_text {
            "Text snippet"
        } else {
            "Preparing…"
        }),
        current_file: String::new(),
        index: 0,
        count: 0,
        bytes_done: 0,
        total_bytes: 0,
        rate_bps: 0,
        eta_seconds: None,
        is_text,
    };

    let cancel = state.register(snapshot.clone()).await;
    let _ = app.emit(events::TRANSFER_PROGRESS, snapshot.clone());

    let task_id = id.clone();
    // Kept so a failure before the transfer loop starts can still drive the row
    // to a terminal state.
    let queued_snapshot = snapshot.clone();

    // Detached: the command that called this has already returned the id, and
    // this task outlives it by however long the transfer takes.
    tokio::spawn(async move {
        // Cloned before `peer` is moved into `send`, for the error path below.
        let alias = peer.alias.clone();
        let result = send(app.clone(), state.clone(), peer, payload, snapshot, cancel).await;

        if let Err(err) = result {
            // `send` records its own outcome once streaming is under way. Paths
            // that fail earlier — cancelled while queued, peer unreachable —
            // return before that, and the UI only retires a row when it sees a
            // terminal phase. Without this the row stays "Queued" forever.
            let mut final_snapshot = queued_snapshot;
            final_snapshot.phase = if matches!(err, TransferError::Cancelled) {
                TransferPhase::Cancelled
            } else {
                TransferPhase::Failed
            };
            let _ = app.emit(events::TRANSFER_PROGRESS, final_snapshot);

            state
                .emit_error(&app, &task_id, Direction::Outgoing, &alias, &err)
                .await;
        }
        // Outside the `if`: the registration must be released whether the
        // transfer succeeded or failed, or the row would linger as active.
        state.finish(&task_id).await;
    });

    id
}

async fn send(
    app: AppHandle,
    state: Arc<AppState>,
    peer: Peer,
    payload: Payload,
    snapshot: TransferSnapshot,
    cancel: CancellationToken,
) -> TransferResult<()> {
    let id = snapshot.id.clone();
    let mut progress = ProgressTracker::new(snapshot);

    // Build the manifest before queueing so the user sees the real size while
    // waiting behind another transfer.
    //
    // Selection on payload shape, mirroring the receive side. Note this runs
    // before the queue locks below — deliberately, since walking a directory
    // tree needs no exclusivity and doing it here means a queued transfer
    // already shows its true item count and size.
    let (offer, entries, mut skipped) = match &payload {
        Payload::Files(roots) => build_files_offer(&app, &state, roots).await?,
        Payload::Text(text) => build_text_offer(&state, text).await?,
    };

    progress.set_label(describe(&offer));
    progress.set_total(offer.total_size);
    progress.set_count(offer.item_count);
    progress.emit(&app, true);
    state.update(progress.snapshot()).await;

    // FR-2.6 — sequential per peer, three peers at a time.
    //
    // FR-2.7 requires cancellation to work "at any time", which includes while
    // queued. Both waits below must therefore race the cancellation token:
    // awaiting them directly parks the task until the transfer ahead of it
    // finishes, so Cancel would appear to do nothing on a queued row.
    // `biased` on both selects, so cancellation is polled first and an already
    // cancelled transfer never acquires the lock it is about to abandon.
    //
    // Two gates in sequence: the per-peer lock (sequential per peer), then a
    // semaphore permit (at most three peers at once). Both guards are bound to
    // `_`-prefixed names so they live to the end of the function and release on
    // every exit path, including `?`.
    let peer_lock = state.peer_lock(&peer.device_id).await;
    let _queue = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(TransferError::Cancelled),
        guard = peer_lock.lock() => guard,
    };

    let slots = state.outbound_slots();
    let _permit = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(TransferError::Cancelled),
        // `acquire_owned` fails only if the semaphore was closed, which happens
        // at shutdown — reported as a cancellation because that is what it is.
        permit = slots.acquire_owned() => permit.map_err(|_| TransferError::Cancelled)?,
    };

    // Acquired only once this transfer is genuinely starting, not while queued:
    // a device waiting its turn is not busy and should not say so (§6.2).
    let _active = state.enter_active();

    let addr: SocketAddr = format!("{}:{}", peer.ip, peer.tcp_port)
        .parse()
        .map_err(|_| TransferError::PeerUnreachable("unparseable peer address".into()))?;

    // Two nested results, two distinct failures, both reported as unreachable:
    // the outer is the 8 s timeout (a peer that has gone to sleep, where TCP
    // would otherwise retry for minutes), the inner an immediate refusal.
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| TransferError::PeerUnreachable("connect timed out".into()))?
        .map_err(|e| TransferError::PeerUnreachable(e.to_string()))?;
    let _ = stream.set_nodelay(true);

    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::with_capacity(SOCKET_BUF, read_half);
    let mut writer = BufWriter::with_capacity(SOCKET_BUF, write_half);

    write_frame(&mut writer, &offer).await?;

    progress.set_phase(TransferPhase::Offered);
    progress.emit(&app, true);
    state.update(progress.snapshot()).await;

    // The peer has OFFER_TIMEOUT to answer; add slack for the round trip.
    //
    // The slack matters: both ends run a 30 s timer, and without it a receiver
    // accepting at 29.9 s would race our own expiry. Erring on the sender's
    // side means the receiver's decision always wins.
    //
    // Raced against cancellation as well, since FR-2.7 allows cancelling while
    // waiting for the other device to answer.
    let mut ack = [0u8; 1];
    let ack_result = tokio::select! {
        _ = cancel.cancelled() => return Err(TransferError::Cancelled),
        r = tokio::time::timeout(OFFER_TIMEOUT + Duration::from_secs(5), reader.read_exact(&mut ack)) => r,
    };

    // Three outcomes, each a different §6.5 code: the timeout elapsed, the
    // socket broke, or a byte arrived. Distinguished because "they didn't
    // answer" and "the connection dropped" are different things to tell a user.
    match ack_result {
        Err(_) => return Err(TransferError::OfferTimeout),
        Ok(Err(e)) => return Err(TransferError::connection(e)),
        Ok(Ok(_)) => {}
    }

    // Anything but ACCEPT is a decline. Tested as `!= ACK_ACCEPT` rather than
    // `== ACK_REJECT` so an unexpected byte is treated conservatively — as a
    // refusal — instead of falling through into streaming data.
    if ack[0] != ACK_ACCEPT {
        // The reason frame is optional in practice: a receiver that dropped
        // immediately after its reject byte leaves nothing to read, so a
        // default stands in rather than turning the decline into an error.
        let reason: RejectReason = read_frame(&mut reader)
            .await
            .unwrap_or_else(|_| RejectReason {
                code: "ERR_REJECTED".into(),
                message: String::new(),
            });
        // The reject frame carries the receiver's own message (e.g. the
        // insufficient-space copy with its byte count), so it is shown verbatim.
        let err = TransferError::Rejected(reason.message);
        record_send_outcome(
            &app,
            &state,
            &id,
            &peer,
            &offer,
            &mut progress,
            Err(&err),
            &[],
        )
        .await;
        return Ok(());
    }

    progress.set_phase(TransferPhase::Transferring);
    progress.emit(&app, true);

    let result = match &payload {
        Payload::Text(text) => stream_text(&app, &mut writer, &mut progress, text, &cancel).await,
        Payload::Files(_) => {
            stream_files(
                &app,
                &mut writer,
                &mut progress,
                &entries,
                &mut skipped,
                &cancel,
            )
            .await
        }
    };

    match result {
        Ok(()) => {
            // §6.3 step 6 — the completion trailer, carrying the true byte
            // count and the skip list the receiver cannot know on its own.
            write_frame(
                &mut writer,
                &CompletionTrailer {
                    status: "complete".into(),
                    bytes_sent: progress.done(),
                    skipped: skipped.clone(),
                },
            )
            .await?;

            // The receiver's acknowledgement, read best-effort: every byte is
            // already delivered and committed, so a peer that closes without
            // acknowledging has still received the transfer. Failing here would
            // report a successful transfer as failed.
            let mut final_ack = [0u8; 1];
            let _ = reader.read_exact(&mut final_ack).await;

            record_send_outcome(
                &app,
                &state,
                &id,
                &peer,
                &offer,
                &mut progress,
                Ok(()),
                &skipped,
            )
            .await;
            Ok(())
        }
        Err(err) => {
            record_send_outcome(
                &app,
                &state,
                &id,
                &peer,
                &offer,
                &mut progress,
                Err(&err),
                &skipped,
            )
            .await;

            // Surfaced here rather than by the caller: the outcome is already
            // recorded with the real byte counts, so returning Err would make
            // the caller emit a second terminal event built from the stale
            // queued snapshot, clobbering it.
            state
                .emit_error(&app, &id, Direction::Outgoing, &peer.alias, &err)
                .await;
            Ok(())
        }
    }
}

/// Where a manifest entry's bytes come from.
///
/// Android can hand out `content://` URIs (share intents, and any SAF picker)
/// for which no filesystem path exists under scoped storage. `PathBuf::from` on
/// one yields a path that cannot be opened, so URIs are kept intact and
/// resolved through `tauri_plugin_fs`, which returns a real file descriptor.
/// Once open, both variants stream identically.
#[derive(Clone)]
pub enum Source {
    Path(PathBuf),
    /// Android content URI, kept as the original string.
    Uri(String),
}

impl Source {
    /// Classify one picker string as a path or a content URI.
    fn parse(raw: &str) -> Self {
        // A Windows drive letter is a one-character "scheme", which is how
        // FilePath distinguishes `C:\foo` from `content://foo`.
        //
        // That guard is the whole subtlety here: `C:\Users\...` parses as a
        // valid URL with scheme `c`, so testing only `Url::parse().is_ok()`
        // would misclassify every absolute Windows path as a URI.
        match url::Url::parse(raw) {
            Ok(url) if url.scheme().len() > 1 => {
                // Prefer a real path whenever the URI maps onto one: a path can
                // be walked (folder sends) and, critically, carries the real
                // filename. A content URI gives us neither.
                //
                // Document URIs are tried first because a pick of a single file
                // inside a granted tree contains both segments.
                match paths::content_document_to_path(raw)
                    .or_else(|| paths::content_tree_to_path(raw))
                {
                    Some(path) => Source::Path(path),
                    // A provider with no filesystem backing (Drive, MediaStore).
                    // Still sendable through the fs plugin — just unwalkable,
                    // and nameless.
                    None => Source::Uri(raw.to_string()),
                }
            }
            // Not a URL at all, or a drive letter: an ordinary path.
            _ => Source::Path(PathBuf::from(raw)),
        }
    }

    fn display(&self) -> String {
        match self {
            Source::Path(p) => p.display().to_string(),
            Source::Uri(u) => u.clone(),
        }
    }

    /// Open for reading, resolving content URIs through the fs plugin.
    fn open(&self, app: &AppHandle) -> std::io::Result<std::fs::File> {
        let file_path: tauri_plugin_fs::FilePath = match self {
            Source::Path(p) => tauri_plugin_fs::FilePath::Path(p.clone()),
            Source::Uri(u) => u
                .parse()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad URI"))?,
        };

        app.fs().open(
            file_path,
            tauri_plugin_fs::OpenOptions::new().read(true).to_owned(),
        )
    }
}

/// One manifest entry plus the source its bytes stream from.
struct SourceEntry {
    manifest: ManifestEntry,
    source: Source,
}

async fn build_files_offer(
    app: &AppHandle,
    state: &Arc<AppState>,
    roots: &[String],
) -> TransferResult<(OfferHeader, Vec<SourceEntry>, Vec<String>)> {
    let mut entries: Vec<SourceEntry> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    // One pass over the picked items. Each root is independently one of three
    // things — a content URI, a file, or a directory — and only the last
    // recurses. A mixed selection of files and folders is ordinary, so the
    // dispatch happens per item rather than once for the whole batch.
    for raw in roots {
        let source = Source::parse(raw);

        // Android content URI: there is no path to stat, so open it and take
        // the size from the descriptor.
        if let Source::Uri(uri) = &source {
            let file = source
                .open(app)
                .map_err(|e| TransferError::read(uri.clone(), e))?;
            // Zero on failure rather than an error: some providers refuse to
            // report a size, and a declared 0 still transfers correctly — the
            // prelude byte and the manifest keep the stream aligned.
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            // Closed immediately; it is reopened when this entry streams. A
            // large batch would otherwise hold every descriptor at once.
            drop(file);

            entries.push(SourceEntry {
                manifest: ManifestEntry {
                    rel_path: uri_file_name(uri),
                    size,
                    is_dir: false,
                },
                source,
            });
            continue;
        }

        // Unreachable because the `if let` above `continue`s on every URI; the
        // binding exists only to name the inner path.
        let Source::Path(root) = &source else {
            unreachable!("Uri handled above")
        };

        // `symlink_metadata`, not `metadata`: the latter follows links, which
        // would report the *target's* type and defeat the check below.
        let meta = tokio::fs::symlink_metadata(root)
            .await
            .map_err(|e| TransferError::read(root, e))?;

        // FR-2.3 — symlinks are never followed. Checked first, because a link
        // to a directory would otherwise be walked.
        if meta.is_symlink() {
            skipped.push(
                root.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
            );
            continue;
        }

        if meta.is_file() {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            entries.push(SourceEntry {
                manifest: ManifestEntry {
                    rel_path: name,
                    size: meta.len(),
                    is_dir: false,
                },
                source: Source::Path(root.clone()),
            });
            continue;
        }

        // Neither a symlink nor a file: recurse. Anything else a filesystem can
        // report (a socket, a device node) falls through all three tests and is
        // silently ignored, which is correct — there is nothing to send.
        if meta.is_dir() {
            walk_dir(root, &mut entries, &mut skipped).await?;
        }
    }

    // Reachable in a way that looks surprising but is not: a folder of nothing
    // but symlinks walks fine and yields an empty manifest. Failing here is
    // what keeps `validate_offer` on the receiving side from having to.
    if entries.is_empty() {
        return Err(TransferError::ReadFailed {
            path: roots.first().cloned().unwrap_or_default(),
            detail: "nothing to send".into(),
        });
    }

    // SEC-2's cap applied to our own manifest, so this device cannot send a
    // header a conforming peer would refuse to read.
    if entries.len() > MAX_MANIFEST_ENTRIES {
        return Err(TransferError::MalformedPayload);
    }

    // Directories excluded from both the byte total and the item count: they
    // carry no data, and counting them would make "3 of 7" disagree with the
    // number of files the user actually sees arrive.
    let total: u64 = entries
        .iter()
        .filter(|e| !e.manifest.is_dir)
        .map(|e| e.manifest.size)
        .sum();

    let settings = state.settings.read().await;
    let offer = OfferHeader {
        protocol_version: PROTOCOL_VERSION,
        payload_type: PayloadType::Files,
        sender_alias: settings.device_alias.clone(),
        sender_device_id: state.identity.read().await.device_id.clone(),
        total_size: total,
        item_count: entries.iter().filter(|e| !e.manifest.is_dir).count(),
        manifest: entries.iter().map(|e| e.manifest.clone()).collect(),
    };

    Ok((offer, entries, skipped))
}

/// FR-2.2 — non-blocking traversal that preserves the tree under the dropped
/// folder's own name.
async fn walk_dir(
    root: &Path,
    entries: &mut Vec<SourceEntry>,
    skipped: &mut Vec<String>,
) -> TransferResult<()> {
    // Every manifest path is prefixed with the dropped folder's own name, so
    // FR-2.2's structure arrives nested rather than spilling the folder's
    // contents loose into the receiver's download directory.
    let base_name = root
        .file_name()
        .map(|n| PathBuf::from(n))
        .unwrap_or_else(|| PathBuf::from("folder"));

    let mut walker = async_walkdir::WalkDir::new(root);
    // Tracks whether the folder had any content at all — see the end of the
    // loop, where an empty folder still needs an entry of its own.
    let mut saw_child = false;

    // Streaming traversal (§9 budgets 50,000 files in 5 s, non-blocking). An
    // unreadable entry is skipped rather than failing the walk: one
    // permission-denied subdirectory should not abort sending the rest.
    while let Some(result) = walker.next().await {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "skipping unreadable entry");
                continue;
            }
        };

        let path = entry.path();
        // Cannot fail while walking under `root`; skipped rather than
        // unwrapped so a surprising path cannot panic the send.
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };

        // Separators normalised to `/` for the wire. The receiver splits on
        // both, but sending the host's native separator would make a manifest
        // built on Windows read differently from one built on Android.
        let rel_path = base_name.join(rel).to_string_lossy().replace('\\', "/");

        // A second fallible call per entry, handled the same way: on Windows
        // the type is not part of the directory listing and needs its own stat.
        let file_type = match entry.file_type().await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "skipping unreadable entry");
                continue;
            }
        };

        // FR-2.3 — symlinks are skipped and reported, not followed.
        // Deliberately before `saw_child`: a folder containing only symlinks
        // counts as empty, and is recreated as an empty directory.
        if file_type.is_symlink() {
            skipped.push(rel_path);
            continue;
        }

        saw_child = true;

        if file_type.is_dir() {
            entries.push(SourceEntry {
                manifest: ManifestEntry {
                    rel_path,
                    size: 0,
                    is_dir: true,
                },
                source: Source::Path(path),
            });
        } else {
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            entries.push(SourceEntry {
                manifest: ManifestEntry {
                    rel_path,
                    size,
                    is_dir: false,
                },
                source: Source::Path(path),
            });
        }

        // Checked inside the walk, not only after it: a runaway directory tree
        // would otherwise be fully enumerated into memory before being
        // rejected. This is the bound that keeps a hostile or pathological
        // folder from exhausting RAM during the build.
        if entries.len() > MAX_MANIFEST_ENTRIES {
            return Err(TransferError::MalformedPayload);
        }
    }

    // FR-2.2 — an empty folder still arrives as a folder.
    // Without this the manifest would be empty and the send would fail with
    // "nothing to send", rather than recreating the folder as asked.
    if !saw_child {
        entries.push(SourceEntry {
            manifest: ManifestEntry {
                rel_path: base_name.to_string_lossy().into_owned(),
                size: 0,
                is_dir: true,
            },
            source: Source::Path(root.to_path_buf()),
        });
    }

    Ok(())
}

async fn build_text_offer(
    state: &Arc<AppState>,
    text: &str,
) -> TransferResult<(OfferHeader, Vec<SourceEntry>, Vec<String>)> {
    let bytes = text.as_bytes().len() as u64;
    if bytes > MAX_TEXT_BYTES {
        // FR-3.4 is handled in the command layer, which converts the snippet to
        // a file before it reaches here. So this branch is unreachable in
        // practice and exists as a backstop: if a future caller bypasses the
        // conversion, this fails locally rather than sending a header the
        // receiver's `validate_offer` would reject as malformed.
        return Err(TransferError::MalformedPayload);
    }

    let settings = state.settings.read().await;
    Ok((
        OfferHeader {
            protocol_version: PROTOCOL_VERSION,
            payload_type: PayloadType::Text,
            sender_alias: settings.device_alias.clone(),
            sender_device_id: state.identity.read().await.device_id.clone(),
            total_size: bytes,
            item_count: 1,
            manifest: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
    ))
}

async fn stream_text<W>(
    app: &AppHandle,
    writer: &mut W,
    progress: &mut ProgressTracker,
    text: &str,
    cancel: &CancellationToken,
) -> TransferResult<()>
where
    W: AsyncWriteExt + Unpin,
{
    progress.begin_item("Text snippet", 1);

    // Chunked even though the whole snippet is already in memory and bounded at
    // 1 MB. Two reasons: it keeps cancellation responsive (checked per chunk
    // rather than once), and it keeps the progress telemetry meaningful instead
    // of jumping 0 → 100 in a single step.
    //
    // No prelude byte here — that framing belongs to file manifests. A text
    // payload's body follows the ACK directly, its length given by
    // `total_size` (§6.3).
    for chunk in text.as_bytes().chunks(CHUNK_SIZE) {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        writer
            .write_all(chunk)
            .await
            .map_err(TransferError::connection)?;
        progress.add_bytes(chunk.len() as u64);
        progress.emit(app, false);
    }

    // Flushed explicitly: the buffered writer would otherwise hold the tail of
    // the snippet while the receiver waited for bytes it had been promised.
    writer.flush().await.map_err(TransferError::connection)?;
    Ok(())
}

async fn stream_files<W>(
    app: &AppHandle,
    writer: &mut W,
    progress: &mut ProgressTracker,
    entries: &[SourceEntry],
    skipped: &mut Vec<String>,
    cancel: &CancellationToken,
) -> TransferResult<()>
where
    W: AsyncWriteExt + Unpin,
{
    // Counted separately from the loop position, because directories are in
    // `entries` but are not items the user is told about. FR-5.1's "n of m"
    // must agree with the item count in the offer header, which also excludes
    // them.
    let mut index = 0usize;

    for entry in entries {
        if cancel.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        // Directories send no prelude and no bytes; the receiver creates them
        // from the manifest alone. Skipping before `index` increments is what
        // keeps the two ends' numbering aligned.
        if entry.manifest.is_dir {
            continue; // directories carry no bytes
        }
        index += 1;

        // §6.6 — the file may have moved or changed since the manifest was
        // built. Open it immediately before streaming: a vanished or resized
        // file is skipped via the prelude byte rather than desynchronizing the
        // stream or silently truncating.
        //
        // Opening (rather than stat-ing a path) is what makes Android work: a
        // content URI has no path to stat, but the fs plugin resolves it to a
        // descriptor. The check is the same either way -- can we open it, and is
        // it still the size we promised?
        let opened = entry.source.open(app);
        // Two conditions in one test: the file opened, *and* it is still
        // exactly the size the manifest declared. The size check is what makes
        // §6.6's "modified mid-transfer" case safe — a file that grew or shrank
        // since the walk cannot be sent under its declared length, and sending
        // a different number of bytes than promised would desynchronise the
        // stream for every entry that follows.
        let usable = matches!(&opened, Ok(f) if f.metadata().map(|m| m.len()) .map(|len| len == entry.manifest.size).unwrap_or(false));

        // The skip path. This is the case the per-entry prelude byte exists
        // for: without it there would be no way to say "this declared entry has
        // no bytes", and the only options would be padding with garbage or
        // dropping the connection and losing the rest of the batch.
        let Some(file) = opened.ok().filter(|_| usable) else {
            writer
                .write_all(&[ENTRY_SKIP])
                .await
                .map_err(TransferError::connection)?;
            skipped.push(entry.manifest.rel_path.clone());
            // Denominator shrinks so the bar still reaches 100% (UI-3).
            progress.set_total(progress.total().saturating_sub(entry.manifest.size));
            progress.emit(app, true);
            continue;
        };

        // ENTRY_DATA — exactly `size` bytes follow.
        writer
            .write_all(&[ENTRY_DATA])
            .await
            .map_err(TransferError::connection)?;

        progress.begin_item(&entry.manifest.rel_path, index);
        progress.emit(app, true);

        let mut file = BufReader::with_capacity(CHUNK_SIZE, File::from_std(file));
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut remaining = entry.manifest.size;

        // The send-side mirror of `write_entry`: same 128 KB reused buffer,
        // same declared-length bound, same per-chunk cancellation check.
        while remaining > 0 {
            if cancel.is_cancelled() {
                return Err(TransferError::Cancelled);
            }
            // Capped at `remaining`, so exactly `size` bytes go out even if the
            // file has since grown — the receiver is reading to that same
            // bound, and sending more would corrupt the next entry.
            let want = remaining.min(CHUNK_SIZE as u64) as usize;
            let n = file
                .read(&mut buf[..want])
                .await
                .map_err(|e| TransferError::read(entry.source.display(), e))?;
            if n == 0 {
                // The file shrank mid-read. We have already promised `size`
                // bytes, so the stream cannot be salvaged.
                //
                // Unlike the pre-open size check above, this cannot be
                // downgraded to a skip: the prelude byte has already been sent,
                // so the receiver is committed to reading `size` bytes and the
                // connection has to fail. §6.6's continue-the-batch behaviour
                // only applies to entries not yet started.
                return Err(TransferError::ReadFailed {
                    path: entry.source.display(),
                    detail: "file changed during transfer".into(),
                });
            }
            writer
                .write_all(&buf[..n])
                .await
                .map_err(TransferError::connection)?;
            remaining -= n as u64;
            // Send-side progress counts bytes handed to the socket. The
            // receiver's own count is the authoritative one for its UI; this is
            // the best a sender can observe.
            progress.add_bytes(n as u64);
            progress.emit(app, false);
        }
    }

    // One flush after the whole manifest, not per entry: the buffered writer
    // coalesces across entry boundaries, and flushing per file would cost a
    // syscall per item on a 5,000-file batch (S5).
    writer.flush().await.map_err(TransferError::connection)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn record_send_outcome(
    app: &AppHandle,
    state: &Arc<AppState>,
    id: &str,
    peer: &Peer,
    offer: &OfferHeader,
    progress: &mut ProgressTracker,
    result: Result<(), &TransferError>,
    skipped: &[String],
) {
    // Four-way selection producing all three values at once, so the phase, the
    // history outcome, and the error payload can never disagree with one
    // another. Cancelled and Rejected are pulled out ahead of the general
    // `Err` arm because §6.4 gives each its own terminal state: the user
    // stopped it, the peer refused it, or it broke — three different things to
    // show, only one of which is a failure.
    let (phase, outcome, error) = match result {
        Ok(()) => (TransferPhase::Completed, Outcome::Completed, None),
        Err(TransferError::Cancelled) => (
            TransferPhase::Cancelled,
            Outcome::Cancelled,
            Some(TransferError::Cancelled.payload(&peer.alias)),
        ),
        Err(TransferError::Rejected(reason)) => (
            TransferPhase::Rejected,
            Outcome::Rejected,
            Some(TransferError::Rejected(reason.clone()).payload(&peer.alias)),
        ),
        Err(err) => (
            TransferPhase::Failed,
            Outcome::Failed,
            Some(err.payload(&peer.alias)),
        ),
    };

    progress.set_phase(phase);
    progress.emit(app, true);

    let entry = HistoryEntry {
        id: id.to_string(),
        direction: Direction::Outgoing,
        peer_alias: peer.alias.clone(),
        peer_device_id: peer.device_id.clone(),
        label: describe(offer),
        item_count: offer.item_count,
        total_bytes: progress.total(),
        outcome,
        finished_at: now_ms(),
        // Both `None` on the sending side: there is nothing local to reveal
        // (the files were already where the user put them) and no snippet body
        // worth storing twice.
        path: None,
        text: None,
        // Present only when something went wrong; the `Ok` arm above set
        // `error` to `None`, so these map to null for a successful transfer.
        error_code: error.as_ref().map(|e| e.code.clone()),
        error_message: error.as_ref().map(|e| e.message.clone()),
        skipped: skipped.to_vec(),
    };

    state.push_history(entry.clone()).await;
    let _ = app.emit(events::TRANSFER_COMPLETE, entry);
}

/// A human label for the batch, used in history and notifications.
fn describe(offer: &OfferHeader) -> String {
    match offer.payload_type {
        PayloadType::Text => "Text snippet".to_string(),
        PayloadType::Files => {
            // The first *file* — directories are skipped, since "Assets + 4
            // more" naming a folder tells the user less than naming a file
            // inside it. Only the basename: a full relative path would not fit
            // a history row.
            let first = offer
                .manifest
                .iter()
                .find(|e| !e.is_dir)
                .map(|e| {
                    e.rel_path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&e.rel_path)
                        .to_string()
                })
                // A manifest of only directories — an empty folder send.
                .unwrap_or_else(|| "Files".into());

            // `0 | 1` share an arm: a single item needs no suffix, and zero is
            // only reachable for a directory-only manifest where `first` is
            // already the generic label. `n - 1` cannot underflow because both
            // small cases are handled above.
            match offer.item_count {
                0 | 1 => first,
                n => format!("{first} + {} more", n - 1),
            }
        }
    }
}

/// Best-effort filename for an Android content URI.
///
/// Content URIs carry no filename: the last segment is usually an opaque
/// provider id such as `document/image%3A1000000034`. A correct display name
/// requires querying `OpenableColumns.DISPLAY_NAME` through Android's
/// ContentResolver, which needs a Kotlin bridge this build does not have yet
/// (see android-kit). Until then, salvage a name when the provider happens to
/// include one, and otherwise fall back to a timestamped generic name so the
/// receiver at least gets something legible rather than `image%3A1000000034`.
fn uri_file_name(uri: &str) -> String {
    // 1. Ask the provider. This is the only authoritative source for URIs that
    //    carry no path, which is most photo and download picks.
    if let Some(name) = crate::android::display_name(uri) {
        if let Some(sanitized) = paths::sanitize_file_name(&name) {
            return sanitized;
        }
    }

    // 2. Some providers put the real name in the last segment.
    let tail = uri.rsplit('/').next().unwrap_or("");
    let decoded = percent_decode(tail);

    // Three conditions distinguish a filename from a provider id, and all are
    // needed. A dot suggests an extension; a colon is the giveaway of a
    // media-provider id (`image:1000000034`), which can also contain a dot; and
    // the final segment has to actually look like an extension — short, and
    // alphanumeric. Without the last test, `1000000034.5` would pass as a name.
    let looks_like_name = decoded.contains('.')
        && !decoded.contains(':')
        && decoded.split('.').next_back().is_some_and(|ext| {
            !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });

    if looks_like_name {
        if let Some(sanitized) = paths::sanitize_file_name(&decoded) {
            return sanitized;
        }
    }

    // 3. Last resort. Media-provider ids are prefixed with their kind
    //    (`image:`, `video:`, `audio:`), so at least give the file an extension
    //    the receiving OS can open rather than a bare name.
    let kind = decoded.split(':').next().unwrap_or("");
    let ext = match kind {
        "image" => ".jpg",
        "video" => ".mp4",
        "audio" => ".m4a",
        _ => "",
    };
    format!("shared-{}{ext}", now_ms())
}

/// Minimal percent-decoding; content URIs only ever escape ASCII here.
///
/// Identical to `paths::percent_decode`. Duplicated rather than shared because
/// that one is private to the path-sanitization boundary, where its behaviour
/// is covered by tests; exporting it would invite reuse in contexts those tests
/// do not describe. Both are eight lines and neither is likely to change.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// A remote alias is untrusted display data: strip control characters and cap
/// the length so it cannot deface the UI. Falls back to the source IP.
fn sanitize_alias(raw: &str, addr: SocketAddr) -> String {
    // Control characters stripped before the length cap, so a peer cannot
    // smuggle newlines or ANSI escapes into a notification or a history row.
    // `take` counts characters, not bytes, so a multi-byte alias is truncated
    // safely.
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(crate::settings::MAX_ALIAS_LEN)
        .collect();
    let cleaned = cleaned.trim().to_string();

    // Falling back to the IP rather than a literal like "Unknown": the address
    // actually identifies the sender, which matters in a prompt asking the user
    // whether to accept files from them.
    if cleaned.is_empty() {
        addr.ip().to_string()
    } else {
        cleaned
    }
}

/// FR-2.9 — free space on the volume holding `path`.
fn available_space(path: &Path) -> Option<u64> {
    use sysinfo::Disks;

    // Refreshed per call rather than cached: a removable volume can be mounted
    // or unmounted between transfers, and this runs once per offer.
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        // Longest matching mount point wins, so `C:\` does not shadow `C:\data`.
        // Without this, a nested mount would be measured against the wrong
        // volume and the free-space check could pass on a full disk.
        .max_by_key(|d| d.mount_point().as_os_str().len())
        // `None` when no mount point matches — the caller then permits the
        // transfer rather than blocking on an unknown.
        .map(|d| d.available_space())
}

/// §6.6 — orphaned `.part` files from a previous crash are swept on launch.
///
/// Necessary because a `.part` file is only deleted by the task that created
/// it. A process killed outright — the OS reclaiming a backgrounded Android
/// app, or a power loss — leaves them behind with nobody to clean up.
pub async fn cleanup_orphans(root: PathBuf) {
    let mut walker = async_walkdir::WalkDir::new(&root);
    let mut removed = 0usize;

    // `while let Some(Ok(_))` stops at the first unreadable entry, which is
    // acceptable for a best-effort sweep: anything missed is caught next launch.
    while let Some(Ok(entry)) = walker.next().await {
        let path = entry.path();
        // Matched on extension alone. Safe because `part_path` appends `.part`
        // to the *whole* filename, so a genuine file called `notes.part` would
        // have been staged as `notes.part.part` — a real user file can only
        // collide here if it was itself named `*.part`, which is the accepted
        // trade-off for not tracking staging state across runs.
        if path.extension().and_then(|e| e.to_str()) == Some("part") {
            if tokio::fs::remove_file(&path).await.is_ok() {
                removed += 1;
            }
        }
    }

    // Logged only when something was actually removed, so a clean startup stays
    // quiet and a recurring crash is visible in the log.
    if removed > 0 {
        tracing::info!(removed, "cleaned up orphaned .part files");
    }
}
