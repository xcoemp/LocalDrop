//! # `protocol.rs` — Wire protocol types and framing (PRD §6.2, §6.3)
//!
//! Control plane: `u32` big-endian length prefix + UTF-8 JSON.
//! Data plane: raw bytes, never JSON-encoded. File boundaries come from the
//! manifest's declared sizes — there is no per-chunk framing.
//!
//! Every constant, type, and limit both ends agree on lives here, so the one
//! place to look when changing the wire format is this file. The TypeScript
//! mirrors in `src/types/protocol.ts` must move with it.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{TransferError, TransferResult};

pub const PROTOCOL_VERSION: u16 = 1;
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// FR-2.4 — 128 KB streaming window.
pub const CHUNK_SIZE: usize = 128 * 1024;
/// SEC-2 — offer headers are size-capped to prevent memory exhaustion.
pub const MAX_HEADER_BYTES: u32 = 1024 * 1024;
/// SEC-2 — manifest entry cap.
pub const MAX_MANIFEST_ENTRIES: usize = 100_000;
/// FR-3.1 / FR-3.4 — snippets above this become a `.txt` file transfer.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// FR-1.1 — heartbeat cadence.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
/// FR-1.3 — prune after 4 missed beats.
pub const PEER_TTL: Duration = Duration::from_secs(12);
/// FR-4.2 — unanswered offers auto-reject.
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for a peer to answer our connection attempt.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

pub const DEFAULT_DISCOVERY_PORT: u16 = 57321;
pub const DEFAULT_TRANSFER_PORT: u16 = 57322;
/// §6.2 — port fallback scans `+1..=+9` before giving up.
pub const PORT_FALLBACK_RANGE: u16 = 9;

pub const ACK_ACCEPT: u8 = 0x01;
pub const ACK_REJECT: u8 = 0x00;

/// Operating system reported by a peer, used for its badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerOs {
    Windows,
    Android,
    Macos,
    Linux,
    /// `#[serde(other)]` — an unrecognised OS string deserializes here instead
    /// of failing the whole heartbeat. A peer running a future build that adds
    /// an OS must still appear on the radar; SEC-5's fail-closed rule is about
    /// malformed *structure*, not an unfamiliar enum label.
    #[serde(other)]
    Unknown,
}

impl PeerOs {
    /// The OS this build is running on.
    ///
    /// Selection over the compile-time target string. Kept as a runtime match
    /// on `consts::OS` rather than `#[cfg]` blocks because the mapping is a
    /// single expression either way, and this version reads as one table.
    pub fn current() -> Self {
        match std::env::consts::OS {
            "windows" => PeerOs::Windows,
            "android" => PeerOs::Android,
            // macOS and Linux are unshipped (§2.2) but the codebase must not
            // preclude them, so they map rather than falling through.
            "macos" => PeerOs::Macos,
            "linux" => PeerOs::Linux,
            _ => PeerOs::Unknown,
        }
    }
}

/// Whether a peer can accept a transfer right now.
///
/// `Busy` renders the card non-droppable rather than hiding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerState {
    Ready,
    Busy,
}

/// §6.2 — UDP heartbeat payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub protocol_version: u16,
    pub device_id: String,
    pub alias: String,
    pub os: PeerOs,
    pub tcp_port: u16,
    pub client_version: String,
    pub state: PeerState,
}

/// Whether a connection carries files or a single text snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadType {
    Files,
    Text,
}

/// One item in an offer. `size` is authoritative: the receiver reads exactly
/// that many bytes and treats any excess as a protocol violation (SEC-3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub rel_path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// §6.3 — offer header, the first frame on every connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferHeader {
    pub protocol_version: u16,
    pub payload_type: PayloadType,
    pub sender_alias: String,
    pub sender_device_id: String,
    pub total_size: u64,
    pub item_count: usize,
    #[serde(default)]
    pub manifest: Vec<ManifestEntry>,
}

/// Sent by the receiver after a `0x00` reject byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectReason {
    pub code: String,
    pub message: String,
}

/// §6.3 — final frame from the sender.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionTrailer {
    pub status: String,
    pub bytes_sent: u64,
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// Write a length-prefixed JSON control frame.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> TransferResult<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|_| TransferError::MalformedPayload)?;

    // SEC-2's cap enforced on the way *out* as well as in. A frame we would
    // refuse to read is one we must not send: better to fail here, with the
    // connection still clean, than to have the peer abort mid-handshake.
    if bytes.len() as u64 > MAX_HEADER_BYTES as u64 {
        return Err(TransferError::MalformedPayload);
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(TransferError::connection)?;
    writer
        .write_all(&bytes)
        .await
        .map_err(TransferError::connection)?;
    writer.flush().await.map_err(TransferError::connection)?;
    Ok(())
}

/// Read a length-prefixed JSON control frame.
///
/// SEC-2 / SEC-5: oversized or malformed frames abort the connection without
/// allocating the declared length.
pub async fn read_frame<R, T>(reader: &mut R) -> TransferResult<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(TransferError::connection)?;
    let len = u32::from_be_bytes(len_buf);

    // SEC-2 / SEC-5 — validated *before* the allocation below, which is the
    // whole point: a hostile peer can claim any length up to 4 GiB in four
    // bytes, and `vec![0u8; len]` would honour it. Rejecting zero too, since an
    // empty frame cannot deserialize into anything and signals a desynchronised
    // stream rather than a valid message.
    if len == 0 || len > MAX_HEADER_BYTES {
        return Err(TransferError::MalformedPayload);
    }

    // Now safe: `len` is bounded by 1 MB.
    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(TransferError::connection)?;
    serde_json::from_slice(&buf).map_err(|_| TransferError::MalformedPayload)
}

/// Validate an incoming offer before any disk work happens (SEC-2, SEC-3).
///
/// Ordered cheapest and most fundamental first, so a hostile or incompatible
/// peer is rejected with the least work done: version, then size caps, then the
/// per-payload-type consistency rules. Nothing here touches the filesystem, and
/// nothing is allocated — this runs before the receiver decides whether even to
/// prompt the user.
pub fn validate_offer(offer: &OfferHeader) -> TransferResult<()> {
    // FR-1.5 / §6.3 — a peer speaking a version we do not implement. Its own
    // distinct error, because this is the one failure here that is not
    // suspicious: it means "update both devices", not "something is wrong".
    if offer.protocol_version != PROTOCOL_VERSION {
        return Err(TransferError::ProtocolMismatch);
    }

    // SEC-2 — the entry cap. Checked before the sum below, which would
    // otherwise iterate an attacker-chosen number of entries.
    if offer.manifest.len() > MAX_MANIFEST_ENTRIES {
        return Err(TransferError::MalformedPayload);
    }

    // Selection on payload type: the two shapes have mutually exclusive
    // invariants, and a header that satisfies neither is malformed.
    match offer.payload_type {
        PayloadType::Text => {
            // A snippet streams its body directly after ACK with its length
            // given by `total_size`, so a manifest is meaningless here — its
            // presence means the sender disagrees with us about the format.
            // The size bound is FR-3.1's 1 MB: anything larger should have been
            // converted to a file transfer by the sender (FR-3.4).
            if !offer.manifest.is_empty() || offer.total_size > MAX_TEXT_BYTES {
                return Err(TransferError::MalformedPayload);
            }
        }
        PayloadType::Files => {
            // A file transfer with nothing in it has no entries to read, so the
            // stream would end where the receiver expects data.
            if offer.manifest.is_empty() {
                return Err(TransferError::MalformedPayload);
            }

            // The declared total must match the manifest, or progress and the
            // free-space check (FR-2.9) are both meaningless.
            //
            // Directories are filtered out because they carry no bytes — FR-2.2
            // recreates them, and they legitimately have `size: 0`.
            let sum: u64 = offer
                .manifest
                .iter()
                .filter(|e| !e.is_dir)
                .map(|e| e.size)
                .sum();
            if sum != offer.total_size {
                return Err(TransferError::MalformedPayload);
            }
        }
    }
    Ok(())
}
