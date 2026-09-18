//! # `settings.rs` — Settings schema and persistence (PRD §11, FR-6)
//!
//! Deviation from §5.2: settings and history are persisted by Rust to JSON in
//! the app config dir rather than through `plugin-store`. The backend owns this
//! state (the discovery loop reads the alias every 3 s), so round-tripping it
//! through the webview would add a race for no benefit.
//!
//! Every read path here is written to degrade rather than fail: a missing file
//! yields defaults, a corrupt one is discarded with a warning, and an
//! unresolvable directory falls back to a temp path. Losing settings is a
//! nuisance; refusing to start over a bad JSON byte is not acceptable for an
//! app whose whole purpose is to be already running when you need it.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_ALIAS_LEN: usize = 32;

/// What to do when an incoming file already exists (FR-2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollisionPolicy {
    Rename,
    Overwrite,
    Skip,
}

impl Default for CollisionPolicy {
    fn default() -> Self {
        CollisionPolicy::Rename
    }
}

/// User-configurable settings (PRD §11), persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub device_alias: String,
    pub download_directory: String,
    /// SEC-6 — off by default, always surfaced in the UI when on.
    pub auto_accept: bool,
    pub auto_copy_snippets: bool,
    pub completion_sound: bool,
    /// FR-6.3's companion — a short tone the moment bytes begin moving.
    ///
    /// Off by default, unlike `completion_sound`. A start cue is most useful to
    /// a receiver running with auto-accept on, where a transfer otherwise begins
    /// with no indication at all; for everyone else it is one more noise per
    /// transfer, and an existing install should not suddenly acquire it.
    pub transfer_start_sound: bool,
    pub launch_minimized: bool,
    pub organize_by_sender: bool,
    pub organize_by_file_type: bool,
    pub collision_policy: CollisionPolicy,
    /// AND-2 — hold a foreground service so the app stays reachable, and
    /// transfers survive, while it is in the background. Android only.
    pub background_service: bool,
    pub discovery_port: u16,
    pub transfer_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_alias: default_alias(),
            download_directory: default_download_dir().to_string_lossy().into_owned(),
            auto_accept: false,
            auto_copy_snippets: true,
            completion_sound: true,
            transfer_start_sound: false,
            launch_minimized: false,
            organize_by_sender: false,
            organize_by_file_type: false,
            collision_policy: CollisionPolicy::Rename,
            background_service: true,
            discovery_port: crate::protocol::DEFAULT_DISCOVERY_PORT,
            transfer_port: crate::protocol::DEFAULT_TRANSFER_PORT,
        }
    }
}

impl Settings {
    /// FR-6.1 — alias is clamped, never rejected, so a paste of a long string
    /// degrades gracefully instead of erroring.
    pub fn normalize(&mut self) {
        // Four independent repairs, each clamping one field rather than
        // rejecting the whole struct. Called on load *and* on every update, so
        // it is the single choke point through which no invalid value passes —
        // whether it came from a hand-edited file, an older build's schema, or
        // the frontend.

        // Also repairs settings written by an earlier build, where Android
        // devices persisted "localhost" as their alias.
        self.device_alias = usable(&self.device_alias).unwrap_or_else(default_alias);

        // An empty download directory would make every received file land in
        // the process's working directory.
        if self.download_directory.trim().is_empty() {
            self.download_directory = default_download_dir().to_string_lossy().into_owned();
        }

        // Port 0 means "any free port" to the OS, which would bind somewhere
        // unpredictable and break the advertised-port contract in §6.2. It is
        // also what a missing field deserializes to under `#[serde(default)]`.
        if self.discovery_port == 0 {
            self.discovery_port = crate::protocol::DEFAULT_DISCOVERY_PORT;
        }
        if self.transfer_port == 0 {
            self.transfer_port = crate::protocol::DEFAULT_TRANSFER_PORT;
        }
    }

    pub fn download_path(&self) -> PathBuf {
        PathBuf::from(&self.download_directory)
    }
}

/// Hostnames that identify nothing. Android reports `localhost` for every
/// device — and reports it *successfully*, so a plain `unwrap_or_else` fallback
/// never fires and every phone would appear on the radar as "localhost".
const USELESS_HOSTNAMES: [&str; 4] = ["localhost", "localhost.localdomain", "android", "unknown"];

fn usable(name: &str) -> Option<String> {
    let trimmed = name.trim();

    // Rejecting both an empty name and a meaningless one in a single guard,
    // since the caller treats them identically — `None` means "try the next
    // source". Lowercased for the comparison so `Localhost` is caught too.
    if trimmed.is_empty() || USELESS_HOSTNAMES.contains(&trimmed.to_ascii_lowercase().as_str()) {
        return None;
    }

    // FR-6.1's 32-character cap, applied by truncation rather than rejection so
    // a long paste degrades instead of erroring. Counted in `chars`, not bytes:
    // slicing a `String` mid-codepoint would panic, and an alias of emoji or
    // CJK characters is entirely legitimate.
    Some(trimmed.chars().take(MAX_ALIAS_LEN).collect())
}

/// FR-6.1 — the name broadcast to peers, in descending order of how much it
/// actually means to a human looking at the radar.
pub fn default_alias() -> String {
    // A chain of fallbacks, each filtered through `usable`, tried in descending
    // order of how much the name means to a human reading the radar. Every
    // link can legitimately fail, which is why this is a chain and not a single
    // call with an `unwrap_or`.

    // The name the user set in Android's own settings, then the model.
    crate::android::device_name()
        .and_then(|name| usable(&name))
        // Desktop: the machine hostname is the recognisable name. Note this is
        // where Android would land on "localhost" without the `usable` filter.
        .or_else(|| whoami::fallible::hostname().ok().and_then(|h| usable(&h)))
        // `devicename` is a different source again, and sometimes populated
        // where the hostname is not.
        .or_else(|| usable(&whoami::devicename()))
        // A literal, so the radar never shows a blank card.
        .unwrap_or_else(|| "LocalDrop Device".to_string())
}

/// AND-4 — on Android the default is app-visible shared storage; an arbitrary
/// absolute path is not writable under scoped storage, which is why the
/// settings UI offers a picker rather than a text field.
pub fn default_download_dir() -> PathBuf {
    // Compile-time selection: exactly one of these blocks exists in any build.
    #[cfg(target_os = "android")]
    {
        // Hardcoded rather than queried: this path is writable because the app
        // creates the files in it itself, which is what makes it work under
        // scoped storage. Verified on device.
        PathBuf::from("/storage/emulated/0/Download/LocalDrop")
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs_download().join("LocalDrop")
    }
}

/// The user's Downloads folder, or the closest usable thing to it.
///
/// Three nested fallbacks, from best to merely functional.
#[cfg(not(target_os = "android"))]
fn dirs_download() -> PathBuf {
    // `USERPROFILE` on Windows, `HOME` elsewhere — tried in that order because
    // this branch is compiled for both and Windows is the shipped target.
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        let downloads = home.join("Downloads");

        // Existence-checked rather than assumed: the folder can be renamed,
        // relocated, or absent on a localised or heavily customised profile.
        if downloads.exists() {
            return downloads;
        }

        // Home itself is a poor default but a real, writable one.
        return home;
    }

    // No home directory at all — a service account, or a stripped environment.
    std::env::temp_dir()
}

/// Stable device identity (§6.2 — `device_id` persists across restarts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub device_id: String,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            device_id: Uuid::new_v4().to_string(),
        }
    }
}

/// JSON-backed persistence for settings, identity, and history.
pub struct Store {
    config_dir: PathBuf,
}

impl Store {
    pub fn new(config_dir: PathBuf) -> Self {
        // Error ignored: if the directory cannot be created, every later read
        // returns defaults and every write silently no-ops, which is exactly
        // the degraded-but-running behaviour this module aims for. Failing here
        // would take the whole app down instead.
        let _ = std::fs::create_dir_all(&config_dir);
        Self { config_dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.config_dir.join(name)
    }

    /// Read settings, falling back to defaults if the file is missing or corrupt.
    ///
    /// Normalized after loading, so a hand-edited or older-schema file is
    /// repaired on the way in rather than propagating an invalid value.
    pub fn load_settings(&self) -> Settings {
        let mut settings: Settings = read_json(&self.path("settings.json")).unwrap_or_default();
        settings.normalize();
        settings
    }

    pub fn save_settings(&self, settings: &Settings) {
        write_json(&self.path("settings.json"), settings);
    }

    /// Read the stable device id, generating and persisting one on first run.
    ///
    /// The `None` arm writes immediately, and that matters: §6.2 requires the
    /// id be stable across restarts, so a generated-but-unsaved id would make
    /// this device appear as a brand-new peer on every launch and leave stale
    /// cards on every other device's radar until their TTLs expired.
    pub fn load_identity(&self) -> Identity {
        let path = self.path("identity.json");
        match read_json::<Identity>(&path) {
            Some(id) => id,
            None => {
                let id = Identity::default();
                write_json(&path, &id);
                id
            }
        }
    }

    /// Issue a new device id, making this device appear as a stranger to peers.
    pub fn reset_identity(&self) -> Identity {
        let id = Identity::default();
        write_json(&self.path("identity.json"), &id);
        id
    }

    pub fn load_history(&self) -> Vec<crate::history::HistoryEntry> {
        read_json(&self.path("history.json")).unwrap_or_default()
    }

    pub fn save_history(&self, history: &[crate::history::HistoryEntry]) {
        write_json(&self.path("history.json"), &history);
    }
}

/// Read and deserialize a state file, or `None`.
///
/// The two failure modes are treated differently on purpose: a missing file is
/// ordinary (first run) and returns `None` silently via `ok()?`, while a file
/// that exists but will not parse is a real anomaly and is logged before being
/// discarded. Both end in defaults, but only one is worth knowing about.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(e) => {
            // A corrupt file must not brick the app; fall back to defaults.
            tracing::warn!(?path, error = %e, "discarding unreadable state file");
            None
        }
    }
}

/// Write atomically — a crash mid-write must not leave an unparseable file.
///
/// Temp file then rename, because `rename` is atomic on both Windows and POSIX:
/// a reader sees either the old file or the new one, never a half-written mix.
/// Writing in place would risk exactly the corrupt file `read_json` has to
/// discard, and settings are written on every toggle.
fn write_json<T: Serialize>(path: &Path, value: &T) {
    // Serialization failing would mean a type that cannot be represented as
    // JSON — a programming error, not a runtime condition. Returning rather
    // than panicking keeps a bug here from killing a transfer.
    let Ok(bytes) = serde_json::to_vec_pretty(value) else {
        return;
    };

    let tmp = path.with_extension("tmp");

    // The rename is attempted only if the write succeeded. Reversed, a failed
    // write followed by an unconditional rename would replace good state with
    // a truncated file — the precise failure this function exists to prevent.
    if std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}
