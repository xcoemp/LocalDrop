//! # `commands.rs` — Tauri command surface (PRD §6.1)
//!
//! Commands are thin: they validate input, touch state, and hand off. Anything
//! that can take longer than a frame is spawned so the webview never blocks.
//!
//! Two consequences of that rule run through the file. First, a command that
//! starts a transfer returns a transfer *id* rather than a result — progress
//! and the outcome arrive later as events, because awaiting a 4 GB send inside
//! an `invoke` would hang the UI thread that issued it. Second, every failure
//! returned here is an [`ErrorPayload`], never a raw Rust error (FR-5.6).
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::discovery::{self, NetworkSnapshot};
use crate::error::{ErrorPayload, TransferError};
use crate::history::HistoryEntry;
use crate::protocol::{PeerOs, MAX_TEXT_BYTES};
use crate::registry::Peer;
use crate::settings::Settings;
use crate::state::{events, AppState, TransferSnapshot};
use crate::transfer::{self, Payload};

type CmdResult<T> = Result<T, ErrorPayload>;

fn fail(err: TransferError) -> ErrorPayload {
    err.payload("the peer")
}

/// Identity of this device, as shown in the header.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub alias: String,
    pub os: PeerOs,
    pub client_version: String,
    pub protocol_version: u16,
    /// §8 — v1.0 transfers are unencrypted. The UI reads this rather than
    /// hardcoding a claim, so the label can never drift from reality.
    pub encrypted: bool,
}

/// This device's identity and protocol capabilities.
#[tauri::command]
pub async fn get_device_info(state: State<'_, Arc<AppState>>) -> CmdResult<DeviceInfo> {
    Ok(DeviceInfo {
        device_id: state.identity.read().await.device_id.clone(),
        alias: state.settings.read().await.device_alias.clone(),
        os: PeerOs::current(),
        client_version: crate::protocol::CLIENT_VERSION.to_string(),
        protocol_version: crate::protocol::PROTOCOL_VERSION,
        encrypted: false,
    })
}

/// Every peer currently on the radar. Seeds the UI; events keep it current.
#[tauri::command]
pub async fn get_peers(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<Peer>> {
    Ok(state.registry.lock().await.list())
}

/// FR-1.6 — clear the table and beat immediately.
#[tauri::command]
pub async fn rescan(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.registry.lock().await.clear();
    let _ = app.emit(events::PEER_LOST, "*");
    state.rescan.notify_waiters();
    Ok(())
}

/// Interface, addresses and bound ports for the diagnostics panel.
#[tauri::command]
pub async fn get_network(state: State<'_, Arc<AppState>>) -> CmdResult<NetworkSnapshot> {
    Ok(discovery::snapshot(&state).await)
}

/// The persisted settings, already normalized.
#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> CmdResult<Settings> {
    Ok(state.settings.read().await.clone())
}

/// FR-6.5 — persisted synchronously on every change.
#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> CmdResult<Settings> {
    // Normalized before anything else, so no unvalidated value from the webview
    // reaches memory or disk. The normalized copy is also what is returned, so
    // the frontend's optimistic value is corrected by the reply.
    let mut next = settings;
    next.normalize();

    {
        // Scoped write lock: released before the disk write below, because
        // `save_settings` is blocking I/O and the discovery loop reads this
        // lock every 3 s.
        let mut current = state.settings.write().await;
        *current = next.clone();
    }
    state.store.save_settings(&next);

    // The next heartbeat (≤ 3 s) carries the new alias — FR-6.1.
    // Waking the loop now makes it immediate rather than up to 3 s late.
    state.rescan.notify_waiters();
    // Broadcast so any other view — the sidebar's auto-accept switch, say —
    // reflects a change made in Settings without polling.
    let _ = app.emit(events::SETTINGS_CHANGED, next.clone());
    Ok(next)
}

/// Restores defaults and regenerates `device_id` (§11).
#[tauri::command]
pub async fn factory_reset(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<Settings> {
    let defaults = Settings::default();
    {
        let mut current = state.settings.write().await;
        *current = defaults.clone();
    }
    state.store.save_settings(&defaults);

    // A new device_id is what makes this a *factory* reset rather than a
    // settings reset: every peer sees this device as a stranger afterwards,
    // and their old cards for it expire on their own TTLs.
    let identity = state.store.reset_identity();
    *state.identity.write().await = identity;

    state.clear_history().await;
    // The local table is cleared too, so the radar is rebuilt under the new
    // identity rather than showing peers that still hold the old one.
    state.registry.lock().await.clear();
    state.rescan.notify_waiters();

    let _ = app.emit(events::SETTINGS_CHANGED, defaults.clone());
    Ok(defaults)
}

/// Resolve a `device_id` to a live peer, or fail with `ERR_PEER_UNREACHABLE`.
///
/// Every send goes through here first. The lookup is not a formality: the
/// frontend's peer list can be up to one TTL stale, so a user clicking Send on
/// a device that has just left the network must get a clear error rather than a
/// connection attempt to a stale address.
async fn require_peer(state: &State<'_, Arc<AppState>>, device_id: &str) -> CmdResult<Peer> {
    state.registry.lock().await.get(device_id).ok_or_else(|| {
        fail(TransferError::PeerUnreachable(
            "peer no longer listed".into(),
        ))
    })
}

/// Queue a file transfer to `device_id` (FR-2.1).
///
/// `paths` are whatever the picker produced: absolute paths from the desktop
/// dialog, an OS drop, or the in-app Android browser. A `content://` URI can
/// still arrive from a share intent, so they are passed through verbatim and
/// [`transfer::Source`] decides how to open each one.
///
/// Returns the transfer id immediately; the transfer itself runs detached.
/// Fails with `ERR_PEER_UNREACHABLE` if the peer has left the network, or
/// `ERR_READ_FAILED` if `paths` is empty.
#[tauri::command]
pub async fn send_files(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    device_id: String,
    paths: Vec<String>,
) -> CmdResult<String> {
    // Checked before the peer lookup: it is the cheaper test, and an empty
    // selection is a caller bug rather than a network condition.
    if paths.is_empty() {
        return Err(fail(TransferError::ReadFailed {
            path: String::new(),
            detail: "no files selected".into(),
        }));
    }

    let peer = require_peer(&state, &device_id).await?;

    // Returns as soon as the transfer is queued — the id, not the outcome.
    Ok(transfer::spawn_send(app, state.inner().clone(), peer, Payload::Files(paths)).await)
}

/// FR-3.1 / FR-3.4 — snippets over 1 MB become a `.txt` file transfer instead.
/// The caller is told which path was taken via `converted`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTextResult {
    pub id: String,
    pub converted: bool,
}

/// Send a text snippet, or a `.txt` file if it exceeds 1 MB (FR-3.1, FR-3.4).
///
/// Fails with `ERR_PEER_UNREACHABLE` if the peer has left, or
/// `ERR_WRITE_FAILED` if the oversize snippet cannot be staged to disk.
#[tauri::command]
pub async fn send_text(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    device_id: String,
    text: String,
) -> CmdResult<SendTextResult> {
    let peer = require_peer(&state, &device_id).await?;

    // FR-3.4's threshold, measured in bytes rather than characters — the wire
    // format's limit is a byte count, and the frontend's `TextEncoder` check
    // measures the same thing so the two agree about what "over 1 MB" means.
    //
    // This is the one branch in the file that changes *which protocol path* is
    // taken: above the limit the snippet becomes an ordinary file transfer, so
    // the receiver needs no special handling for large text at all.
    if text.as_bytes().len() as u64 > MAX_TEXT_BYTES {
        // Staged in the temp dir; the receiver sees an ordinary file transfer.
        // Timestamped so two oversize snippets sent in the same session cannot
        // collide on the staging path.
        let stamp = crate::history::now_ms();
        let path = std::env::temp_dir().join(format!("LocalDrop-snippet-{stamp}.txt"));
        tokio::fs::write(&path, text.as_bytes())
            .await
            .map_err(|e| fail(TransferError::write(&path, e)))?;

        let id = transfer::spawn_send(
            app,
            state.inner().clone(),
            peer,
            Payload::Files(vec![path.to_string_lossy().into_owned()]),
        )
        .await;
        return Ok(SendTextResult {
            id,
            converted: true,
        });
    }

    let id = transfer::spawn_send(app, state.inner().clone(), peer, Payload::Text(text)).await;
    Ok(SendTextResult {
        id,
        converted: false,
    })
}

/// FR-4.1 — the user's answer to an incoming offer.
#[tauri::command]
pub async fn respond_to_offer(
    state: State<'_, Arc<AppState>>,
    id: String,
    accept: bool,
) -> CmdResult<bool> {
    Ok(state.answer_offer(&id, accept).await)
}

/// FR-2.7 — cancellable from either end, at any point.
#[tauri::command]
pub async fn cancel_transfer(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<bool> {
    Ok(state.cancel(&id).await)
}

/// Transfers currently in flight, for restoring UI state on mount.
#[tauri::command]
pub async fn get_active_transfers(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<TransferSnapshot>> {
    Ok(state.active_transfers().await)
}

/// Completed transfers, newest first (FR-5.4).
#[tauri::command]
pub async fn get_history(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<HistoryEntry>> {
    Ok(state.history().await)
}

/// Drop every history entry and persist the empty list.
#[tauri::command]
pub async fn clear_history(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.clear_history().await;
    Ok(())
}

/// One row in the in-app file browser.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// A browsed directory: its own path, its parent, and its contents.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntryInfo>,
}

/// Browse the filesystem directly, for the in-app picker on Android.
///
/// Android's SAF picker returns `content://` URIs. Media-provider ids carry no
/// filename at all, so a photo picked through the system picker can only be
/// saved as `shared-<timestamp>`; recovering the real name would need an
/// `OpenableColumns.DISPLAY_NAME` query from Java. With all-files access
/// granted, browsing real paths sidesteps that entirely: every pick arrives
/// with its true name, and folders can be walked.
///
/// `path` of `None` starts at the platform's natural root.
#[tauri::command]
pub async fn list_directory(path: Option<String>) -> CmdResult<DirListing> {
    // Guarded pattern: a present-but-blank string is treated as absent. The
    // frontend sends `null` for the root, but an empty string would otherwise
    // become `PathBuf::from("")`, which reads as the current directory.
    let dir = match path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => browse_root(),
    };

    let mut read_dir = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| fail(TransferError::read(&dir, e)))?;

    let mut entries: Vec<DirEntryInfo> = Vec::new();

    // `while let Ok(Some(_))` stops on *either* the end of the directory or a
    // read error. Ending early on an error is deliberate: a partial listing is
    // more useful than none, and the entries gathered so far are still valid.
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();

        // Dotfiles are noise in a send picker.
        if name.starts_with('.') {
            continue;
        }

        // `metadata` follows symlinks, matching what a send would actually read.
        // Entries whose metadata cannot be read are skipped rather than listed:
        // on Android several system paths deny `stat` even with all-files
        // access, and offering them would only produce a failure on tap.
        let Ok(meta) = entry.metadata().await else {
            continue;
        };

        entries.push(DirEntryInfo {
            name,
            path: entry.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            // Zero for directories rather than a recursive total: walking every
            // subtree to size a listing would make browsing unusably slow.
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }

    // Directories first, then case-insensitive by name.
    //
    // `b.cmp(&a)` on `is_dir` reverses the boolean order so `true` sorts first.
    // Lowercased for the name comparison, because a raw byte sort would put
    // every capitalised name above every lowercase one — technically ordered,
    // but not the order a person scanning the list expects.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(DirListing {
        path: dir.to_string_lossy().into_owned(),
        parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
        entries,
    })
}

/// Where the in-app browser starts, and the highest point it can reach.
///
/// This is also the only thing confining the browser: `list_directory` walks
/// wherever it is pointed, and the frontend only ever offers the parent of the
/// current listing. Starting at the user's own storage rather than `/` means
/// the natural upward path stops somewhere meaningful.
fn browse_root() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        // The shared-storage volume, not the app's private directory: the
        // point of the browser is to reach the user's own files.
        PathBuf::from("/storage/emulated/0")
    }
    #[cfg(not(target_os = "android"))]
    {
        // Home, falling back to the filesystem root. Never fails, so the
        // browser always opens somewhere.
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
}

/// State of the Android background listener, for the Settings UI.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStatus {
    /// Whether the foreground service should be running.
    pub enabled: bool,
    /// Whether the app is exempt from Doze battery optimisation.
    ///
    /// When false, long transfers can still be frozen between Doze maintenance
    /// windows even with the service running.
    pub battery_unrestricted: bool,
}

/// Start the Android foreground service so transfers survive backgrounding
/// (AND-2), and remember the choice.
///
/// No-op on desktop, where processes are not suspended when unfocused.
#[tauri::command]
pub async fn start_background_listener(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> CmdResult<BackgroundStatus> {
    set_background_service(&app, &state, true).await;
    crate::android::start_background_service(None);

    Ok(BackgroundStatus {
        enabled: true,
        battery_unrestricted: crate::android::is_ignoring_battery_optimizations(),
    })
}

/// Stop the foreground service, releasing its wake lock and notification.
///
/// The app keeps listening while it is in the foreground; it simply loses the
/// protection that keeps it alive once backgrounded.
#[tauri::command]
pub async fn stop_background_listener(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> CmdResult<BackgroundStatus> {
    set_background_service(&app, &state, false).await;
    crate::android::stop_background_service();

    Ok(BackgroundStatus {
        enabled: false,
        battery_unrestricted: crate::android::is_ignoring_battery_optimizations(),
    })
}

#[tauri::command]
pub async fn get_background_status(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<BackgroundStatus> {
    Ok(BackgroundStatus {
        enabled: state.settings.read().await.background_service,
        battery_unrestricted: crate::android::is_ignoring_battery_optimizations(),
    })
}

/// Open the system battery-optimisation exemption prompt.
///
/// Must be triggered by the user: the dialog is disruptive, and Play restricts
/// apps that request the exemption without a qualifying use case.
#[tauri::command]
pub async fn request_battery_exemption() -> CmdResult<()> {
    crate::android::request_ignore_battery_optimizations();
    Ok(())
}

/// Persist the background-service preference, shared by both toggles.
///
/// Read-clone-write rather than mutating under one lock, so the disk write and
/// the event emit happen outside the write guard.
async fn set_background_service(app: &AppHandle, state: &State<'_, Arc<AppState>>, on: bool) {
    let mut next = state.settings.read().await.clone();
    next.background_service = on;

    {
        let mut current = state.settings.write().await;
        *current = next.clone();
    }
    state.store.save_settings(&next);
    let _ = app.emit(events::SETTINGS_CHANGED, next);
}

/// FR-5.5 — lets the UI disable `Show in Folder` for entries whose file moved.
///
/// Errors collapse to `false`: a path that cannot be tested is, for the UI's
/// purposes, one that cannot be revealed either.
#[tauri::command]
pub async fn path_exists(path: String) -> CmdResult<bool> {
    Ok(tokio::fs::try_exists(&path).await.unwrap_or(false))
}

/// FR-6.2 — set the download directory, proving it is usable before saving.
///
/// Android's folder picker hands back a SAF tree URI rather than a path. Where
/// it maps onto real storage we resolve it; where it does not (Drive, Dropbox)
/// we refuse rather than storing a value that would fail at transfer time with
/// a baffling error. Writability is then proven with a probe file, because a
/// resolvable path is not necessarily a writable one under scoped storage.
#[tauri::command]
pub async fn set_download_directory(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
) -> CmdResult<Settings> {
    // Two input shapes: a SAF tree URI from Android's picker, or a real path
    // from the desktop dialog and the in-app browser. Discriminated by scheme
    // because only Android produces the former.
    let resolved = if path.starts_with("content://") {
        // A bespoke error code rather than one of §6.5's: this is a
        // configuration refusal, not a transfer failure, and the message has to
        // tell the user what to do instead. Refusing now is the whole point —
        // storing an unusable URI would surface as a baffling write error
        // partway through a future transfer.
        crate::paths::content_tree_to_path(&path).ok_or_else(|| ErrorPayload {
            code: "ERR_UNSUPPORTED_FOLDER".into(),
            message: "That folder lives in an app like Drive or Dropbox, which LocalDrop can't \
                      write to directly. Pick a folder on this device's storage."
                .into(),
            detail: Some(path.clone()),
        })?
    } else {
        PathBuf::from(&path)
    };

    // Created first: the user may have picked a folder that does not exist yet,
    // and this also fails early on a path that cannot be created at all.
    tokio::fs::create_dir_all(&resolved)
        .await
        .map_err(|e| fail(TransferError::write(&resolved, e)))?;

    // Prove it, rather than assuming. Under scoped storage a directory can be
    // creatable and listable but not writable, so only an actual write settles
    // it. FR-6.2 requires the user find out now, not mid-transfer.
    let probe = resolved.join(".localdrop-write-test");
    tokio::fs::write(&probe, b"ok")
        .await
        .map_err(|e| fail(TransferError::write(&resolved, e)))?;
    // Cleanup is best-effort: a leftover probe file is harmless, and failing
    // the whole operation over it would reject a folder that just passed.
    let _ = tokio::fs::remove_file(&probe).await;

    let mut next = state.settings.read().await.clone();
    next.download_directory = resolved.to_string_lossy().into_owned();
    next.normalize();

    {
        let mut current = state.settings.write().await;
        *current = next.clone();
    }
    state.store.save_settings(&next);
    let _ = app.emit(events::SETTINGS_CHANGED, next.clone());

    Ok(next)
}

/// FR-6.2 — the effective download directory, created on demand so the first
/// transfer never fails on a missing folder.
#[tauri::command]
pub async fn ensure_download_dir(state: State<'_, Arc<AppState>>) -> CmdResult<String> {
    let dir = state.settings.read().await.download_path();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| fail(TransferError::write(&dir, e)))?;
    Ok(dir.to_string_lossy().into_owned())
}
