//! # `error.rs` — Error taxonomy (PRD §6.5)
//!
//! Every failure maps to exactly one code. The frontend renders `message`;
//! logs carry `code` and `detail`. A raw Rust error string never reaches the UI
//! (FR-5.6) — it goes to `detail`, which is only shown behind "copy details".
//!
//! Three parallel `match`es over the same enum follow: [`TransferError::code`],
//! [`TransferError::message`], and [`TransferError::detail`]. They are kept
//! separate rather than folded into one method returning a tuple, because each
//! has a different audience — a support code, a sentence for the user, and a
//! technical string for the log — and because an added variant then produces
//! three distinct compiler errors, one per thing that must be written.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use serde::Serialize;
use thiserror::Error;

pub type TransferResult<T> = Result<T, TransferError>;

/// Every failure the transfer engine can produce.
///
/// One variant per user-visible cause, so the UI never has to parse a string.
/// Payload detail is kept separate from the message shown to the user.
#[derive(Debug, Error)]
pub enum TransferError {
    #[error("peer unreachable")]
    PeerUnreachable(String),

    #[error("connection lost")]
    ConnectionLost(String),

    #[error("rejected by peer")]
    Rejected(String),

    #[error("offer timed out")]
    OfferTimeout,

    #[error("insufficient space")]
    InsufficientSpace { needed: u64, available: u64 },

    #[error("write failed")]
    WriteFailed { path: String, detail: String },

    #[error("read failed")]
    ReadFailed { path: String, detail: String },

    #[error("protocol mismatch")]
    ProtocolMismatch,

    #[error("malformed payload")]
    MalformedPayload,

    #[error("port unavailable")]
    PortUnavailable { first: u16, last: u16 },

    #[error("cancelled")]
    Cancelled,
}

impl TransferError {
    /// Wrap a socket error, preserving the OS detail for the log.
    pub fn connection(e: std::io::Error) -> Self {
        TransferError::ConnectionLost(e.to_string())
    }

    /// Wrap a failure writing to `path`.
    pub fn write(path: impl AsRef<std::path::Path>, e: std::io::Error) -> Self {
        TransferError::WriteFailed {
            path: path.as_ref().display().to_string(),
            detail: e.to_string(),
        }
    }

    /// Wrap a failure reading `path`.
    pub fn read(path: impl AsRef<std::path::Path>, e: std::io::Error) -> Self {
        TransferError::ReadFailed {
            path: path.as_ref().display().to_string(),
            detail: e.to_string(),
        }
    }

    /// Stable identifier for logs and support, e.g. `ERR_CONNECTION_LOST`.
    ///
    /// Exhaustive with no wildcard arm, so adding a variant is a compile error
    /// rather than a silently mislabelled failure. These strings are quoted in
    /// the user guide and in bug reports, so they must never be renamed.
    pub fn code(&self) -> &'static str {
        match self {
            TransferError::PeerUnreachable(_) => "ERR_PEER_UNREACHABLE",
            TransferError::ConnectionLost(_) => "ERR_CONNECTION_LOST",
            TransferError::Rejected(_) => "ERR_REJECTED",
            TransferError::OfferTimeout => "ERR_OFFER_TIMEOUT",
            TransferError::InsufficientSpace { .. } => "ERR_INSUFFICIENT_SPACE",
            TransferError::WriteFailed { .. } => "ERR_WRITE_FAILED",
            TransferError::ReadFailed { .. } => "ERR_READ_FAILED",
            TransferError::ProtocolMismatch => "ERR_PROTOCOL_MISMATCH",
            TransferError::MalformedPayload => "ERR_MALFORMED_PAYLOAD",
            TransferError::PortUnavailable { .. } => "ERR_PORT_UNAVAILABLE",
            TransferError::Cancelled => "ERR_CANCELLED",
        }
    }

    /// User-facing copy, with `{alias}` resolved. §6.5 table.
    ///
    /// Every string names the peer or the path involved, because "transfer
    /// failed" on a device with three peers on the radar is not actionable.
    pub fn message(&self, alias: &str) -> String {
        match self {
            TransferError::PeerUnreachable(_) => {
                format!("Couldn't reach {alias} — it may have left the network.")
            }
            TransferError::ConnectionLost(_) => {
                format!("Connection to {alias} dropped mid-transfer.")
            }
            // Guarded arm, and the only one in this match: a receiver may send
            // a reason with its rejection (§6.3's reject frame), and that
            // reason is more specific than anything this side could phrase.
            // Falls through to the generic wording when the reason is absent —
            // which is the common case, since declining is just a button.
            TransferError::Rejected(reason) if !reason.is_empty() => reason.clone(),
            TransferError::Rejected(_) => format!("{alias} declined the transfer."),
            TransferError::OfferTimeout => format!("{alias} didn't respond in time."),
            TransferError::InsufficientSpace { needed, .. } => format!(
                "{alias} doesn't have enough free space ({} required).",
                human_bytes(*needed)
            ),
            TransferError::WriteFailed { path, .. } => {
                format!("Couldn't save to {path}. Check permissions.")
            }
            TransferError::ReadFailed { path, .. } => {
                format!("Couldn't read {path} — it may have been moved or deleted.")
            }
            TransferError::ProtocolMismatch => {
                format!("{alias} is running an incompatible version of LocalDrop.")
            }
            TransferError::MalformedPayload => {
                format!("Received invalid data from {alias}. Transfer aborted.")
            }
            TransferError::PortUnavailable { first, last } => {
                format!("Ports {first}–{last} are in use. LocalDrop can't start networking.")
            }
            TransferError::Cancelled => "Transfer cancelled.".to_string(),
        }
    }

    /// The technical detail behind the failure, for logs and bug reports.
    ///
    /// Unlike `code` and `message` this one *does* use a wildcard arm, and
    /// that is the right default: a new variant with no extra context should
    /// report none rather than force an invented string. The variants below are
    /// grouped by payload shape, so each `Some` is written once.
    fn detail(&self) -> Option<String> {
        match self {
            // Three variants that each carry a single opaque string — an OS
            // error, or a reason from the peer.
            TransferError::PeerUnreachable(d)
            | TransferError::ConnectionLost(d)
            | TransferError::Rejected(d) => Some(d.clone()),
            // Two that carry a path and a detail; the path is already in the
            // user-facing message, so only the detail is added here.
            TransferError::WriteFailed { detail, .. }
            | TransferError::ReadFailed { detail, .. } => Some(detail.clone()),
            // Exact byte figures, deliberately unformatted: the message shows
            // the human-readable requirement, the log keeps the precise numbers.
            TransferError::InsufficientSpace { needed, available } => {
                Some(format!("needed {needed} bytes, {available} available"))
            }
            // The remaining variants are fully described by their code alone.
            _ => None,
        }
    }

    /// Build the frontend-facing payload, resolving `{alias}` in the message.
    pub fn payload(&self, alias: &str) -> ErrorPayload {
        ErrorPayload {
            code: self.code().to_string(),
            message: self.message(alias),
            detail: self.detail(),
        }
    }
}

/// Serialized shape delivered to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
}

impl serde::Serialize for TransferError {
    /// Serializes as an [`ErrorPayload`], so a `TransferError` returned from a
    /// Tauri command reaches the frontend in the shape it expects.
    ///
    /// "the peer" stands in for the alias because this path has no peer in
    /// scope — it is used by commands that fail before a destination is known.
    /// Anywhere the alias *is* known, `payload(alias)` is called explicitly.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.payload("the peer").serialize(s)
    }
}

/// Format a byte count for display, e.g. `1.4 GB`.
///
/// Duplicates `formatBytes` in the frontend's `useFormat.ts`, and the
/// duplication is deliberate: this is only reached for the one error message
/// that embeds a size (`ERR_INSUFFICIENT_SPACE`), and routing that through the
/// webview to be formatted would mean the backend could not log a readable
/// figure on its own.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut value = bytes as f64;
    let mut unit = 0;

    // Repetition: step up the unit ladder until the value is under 1024 or the
    // units run out. The second condition prevents indexing past `TB` — without
    // it, an implausible but representable `u64` would panic on the index below.
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    // Bytes print as a whole number from the original integer, avoiding both a
    // pointless ".0" and any float rounding; larger units keep one decimal.
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
