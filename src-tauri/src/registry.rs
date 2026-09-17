//! # `registry.rs` — Peer table with TTL pruning (PRD FR-1.2, FR-1.3, §6.6)
//!
//! The authoritative answer to "which devices are on the LAN right now".
//! Heartbeats arriving on UDP 57321 are absorbed here; anything unheard from
//! for longer than [`PEER_TTL`] is dropped.
//!
//! The module is synchronous and lock-free by design — it is always reached
//! through the `Mutex` in [`crate::state`], so it can keep plain `HashMap`
//! access and leave concurrency to its single owner.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;

use serde::Serialize;

use crate::protocol::{Heartbeat, PeerOs, PeerState, PEER_TTL};

/// A peer currently visible on the LAN.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub device_id: String,
    pub alias: String,
    pub os: PeerOs,
    pub ip: String,
    pub tcp_port: u16,
    pub client_version: String,
    pub state: PeerState,
    /// Milliseconds since this peer was last heard from; the UI uses it for the
    /// "fading" treatment as a peer approaches the prune threshold (FR-1.3).
    pub last_seen_ms: u64,
}

struct Entry {
    peer: Peer,
    last_seen: Instant,
}

/// The live peer table, keyed by `device_id` and pruned on a TTL (FR-1.3).
#[derive(Default)]
pub struct PeerRegistry {
    peers: HashMap<String, Entry>,
}

/// Outcome of absorbing a heartbeat, so the caller only emits events that
/// change something the user can see.
pub enum Upsert {
    /// First time we have seen this `device_id` — the frontend should announce it.
    Added(Peer),
    /// Already known; refreshed in place. Emitted only when something the user
    /// can see actually changed, to avoid a 3 s repaint cycle per peer.
    Updated(Peer),
    /// Heartbeat absorbed with no user-visible change.
    Unchanged,
}

impl PeerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a heartbeat, adding the peer or refreshing it in place.
    ///
    /// The two branches are "known peer" and "new peer". Keying on `device_id`
    /// rather than on the source address is what makes §6.6's dual-homed case
    /// work: the same device reaching us from a second interface is still one
    /// device, and must not become a second card.
    pub fn upsert(&mut self, hb: Heartbeat, ip: IpAddr) -> Upsert {
        let now = Instant::now();
        let ip = ip.to_string();

        if let Some(existing) = self.peers.get_mut(&hb.device_id) {
            // Compared *before* the fields are overwritten, and only across the
            // fields a user can actually see. This is what suppresses a repaint
            // every 3 s for every peer: a heartbeat that says nothing new
            // returns `Unchanged` and emits no event, so the radar stays still
            // while the table underneath it keeps refreshing.
            //
            // §6.6 — a dual-homed host keeps a single card at its newest IP,
            // which is why `ip` counts as a visible change rather than being
            // ignored or treated as a conflict.
            let changed = existing.peer.alias != hb.alias
                || existing.peer.ip != ip
                || existing.peer.state != hb.state
                || existing.peer.tcp_port != hb.tcp_port;

            // Everything is overwritten regardless of `changed`. `os` and
            // `client_version` are deliberately outside the comparison above —
            // they are recorded for display but cannot realistically change for
            // a given device id, so they are not worth a repaint.
            existing.peer.alias = hb.alias;
            existing.peer.ip = ip;
            existing.peer.state = hb.state;
            existing.peer.tcp_port = hb.tcp_port;
            existing.peer.os = hb.os;
            existing.peer.client_version = hb.client_version;
            // Zeroed here and recomputed on read — see `get` and `list`.
            existing.peer.last_seen_ms = 0;
            // The TTL clock, reset by every heartbeat including an unchanged
            // one. This is the line that actually keeps the peer alive.
            existing.last_seen = now;

            return if changed {
                Upsert::Updated(existing.peer.clone())
            } else {
                Upsert::Unchanged
            };
        }

        // First sighting: build the peer and report it as added, which is what
        // FR-1.2 requires reach the UI within 200 ms.
        let peer = Peer {
            device_id: hb.device_id.clone(),
            alias: hb.alias,
            os: hb.os,
            ip,
            tcp_port: hb.tcp_port,
            client_version: hb.client_version,
            state: hb.state,
            last_seen_ms: 0,
        };
        self.peers.insert(
            hb.device_id,
            Entry {
                peer: peer.clone(),
                last_seen: now,
            },
        );
        Upsert::Added(peer)
    }

    /// FR-1.3 — drop peers past the TTL, returning their ids so the frontend
    /// can animate them out.
    pub fn prune(&mut self) -> Vec<String> {
        let now = Instant::now();

        // Two passes, deliberately: collect the expired ids first, then remove
        // them. A single `retain` would be shorter but could not hand the ids
        // back, and the caller needs them to emit one `peer://lost` per peer so
        // the UI can animate each card out (FR-1.3).
        //
        // `now` is captured once rather than calling `Instant::now()` per entry,
        // so every peer is judged against the same instant.
        let expired: Vec<String> = self
            .peers
            .iter()
            .filter(|(_, e)| now.duration_since(e.last_seen) > PEER_TTL)
            .map(|(id, _)| id.clone())
            .collect();

        // Second pass: the actual removal. Separate from the filter above
        // because iterating a map while mutating it is not permitted.
        for id in &expired {
            self.peers.remove(id);
        }
        expired
    }

    /// FR-1.6 — rescan rebuilds the table from scratch.
    pub fn clear(&mut self) {
        self.peers.clear();
    }

    /// Look up a peer, with `last_seen_ms` computed at call time.
    ///
    /// Derived on read rather than stored, because it is a duration that keeps
    /// growing: a cached number would be stale the moment after it was written.
    pub fn get(&self, device_id: &str) -> Option<Peer> {
        self.peers.get(device_id).map(|e| {
            let mut peer = e.peer.clone();
            peer.last_seen_ms = e.last_seen.elapsed().as_millis() as u64;
            peer
        })
    }

    /// Every known peer, ordered stably so cards do not shuffle between polls.
    pub fn list(&self) -> Vec<Peer> {
        // One pass to clone and stamp each peer with its freshness, same as
        // `get`. Cloning is acceptable here: a LAN's peer count is small, and
        // handing out owned values keeps the registry's lock held only for the
        // duration of this call.
        let mut peers: Vec<Peer> = self
            .peers
            .values()
            .map(|e| {
                let mut peer = e.peer.clone();
                peer.last_seen_ms = e.last_seen.elapsed().as_millis() as u64;
                peer
            })
            .collect();

        // Stable ordering so cards never shuffle between polls (UI-3 spirit).
        // `HashMap` iteration order is arbitrary and varies run to run, so
        // without this the grid would reorder itself on every refresh.
        //
        // Alias first because that is what the user reads; device id as the
        // tiebreak, since §6.6 explicitly allows two devices to share an alias
        // and the comparison has to be total for the order to be stable.
        peers.sort_by(|a, b| a.alias.cmp(&b.alias).then(a.device_id.cmp(&b.device_id)));
        peers
    }
}
