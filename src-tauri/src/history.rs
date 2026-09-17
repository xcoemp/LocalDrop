//! # `history.rs` — Transfer history (PRD FR-5.4, FR-5.5)
//!
//! A capped, newest-first log of finished transfers, persisted across restarts
//! by [`crate::settings`]. Deliberately data-only: the module holds the schema
//! and one insertion rule, and every decision about *when* to record an entry
//! lives in [`crate::transfer`] where the outcome is known.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use serde::{Deserialize, Serialize};

/// FR-5.4 — capped so the file cannot grow without bound.
pub const HISTORY_LIMIT: usize = 200;

/// Which way a transfer went, from this device's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Incoming,
    Outgoing,
}

/// How a transfer ended. Anything but `Completed` carries an error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
    Rejected,
}

/// A finished transfer, persisted across restarts (FR-5.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub direction: Direction,
    pub peer_alias: String,
    pub peer_device_id: String,
    pub label: String,
    pub item_count: usize,
    pub total_bytes: u64,
    pub outcome: Outcome,
    /// Unix milliseconds — the frontend formats it for display.
    pub finished_at: u64,
    /// Absolute path for `Open File` / `Show in Folder` (FR-5.5). `None` for
    /// outgoing transfers of multiple items and for text snippets.
    pub path: Option<String>,
    /// Text snippet body, retained so it stays copyable from history (FR-3.3).
    pub text: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub skipped: Vec<String>,
}

/// Unix milliseconds; the frontend formats these for display.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        // `duration_since` fails only if the system clock is set before 1970.
        // Falling back to 0 rather than panicking: a nonsensical timestamp on a
        // history row is a cosmetic problem, and aborting a completed transfer
        // over a misconfigured clock would not be.
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Newest first, trimmed to the cap.
pub fn push(history: &mut Vec<HistoryEntry>, entry: HistoryEntry) {
    // `insert(0, …)` rather than `push`, because FR-5.4's list is read
    // newest-first and the cost is irrelevant at 200 entries — an O(n) memmove
    // of a bounded vector, once per completed transfer.
    history.insert(0, entry);
    // Trimming after every insertion keeps the invariant local: the vector can
    // never exceed the cap, so no caller and no deserialization path has to
    // check. `truncate` is a no-op while the history is still short.
    history.truncate(HISTORY_LIMIT);
}
