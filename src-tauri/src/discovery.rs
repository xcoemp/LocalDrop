//! # `discovery.rs` — UDP broadcast discovery (PRD §6.2, FR-1)
//!
//! Three loops run against one bound socket: a 3 s heartbeat, a receive loop,
//! and a 2 s pruner. A supervisor watches the interface list and rebinds the
//! whole set when the network changes (FR-1.8).
//!
//! ## Why four loops rather than one
//!
//! Each has an independent cadence and an independent failure mode. Folding
//! them together would tie the heartbeat's 3 s timer to however long a
//! `recv_from` happened to block, so a quiet network would delay our own
//! announcements. They coordinate through a single [`CancellationToken`]: when
//! the supervisor decides to rebind, cancelling it stops all three at their
//! next await point, and the sockets they hold are dropped with them.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::error::{TransferError, TransferResult};
use crate::protocol::{
    Heartbeat, PeerOs, PeerState, HEARTBEAT_INTERVAL, PORT_FALLBACK_RANGE, PROTOCOL_VERSION,
};
use crate::registry::Upsert;
use crate::state::{events, AppState};

/// Discovery datagrams are small; anything larger is not ours.
const RECV_BUF: usize = 4096;
const PRUNE_INTERVAL: Duration = Duration::from_secs(2);
/// FR-1.8 — how often the interface list is polled for changes.
const NETWORK_POLL: Duration = Duration::from_secs(3);

/// Current network state, surfaced in the header and Settings diagnostics.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
    pub online: bool,
    pub interface: Option<String>,
    pub local_ip: Option<String>,
    pub netmask: Option<String>,
    pub broadcast_targets: Vec<String>,
    pub discovery_port: u16,
    pub transfer_port: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IncompatiblePeer {
    device_id: String,
    alias: String,
    ip: String,
    client_version: String,
    protocol_version: u16,
}

/// Supervisor: binds, runs the loops, and rebinds on interface change.
///
/// The outer loop never exits — it runs for the life of the process. Each
/// iteration is one "binding generation": bind, start the three workers, wait
/// for the network to change, tear them down, repeat. Structuring it this way
/// means a Wi-Fi-to-Ethernet switch is handled by the same code path as
/// startup, rather than needing a separate rebind routine (FR-1.8).
pub async fn run(app: AppHandle, state: Arc<AppState>) {
    loop {
        // Re-read each generation, so a port changed in settings takes effect
        // on the next rebind without restarting the app.
        let port = state.settings.read().await.discovery_port;

        let socket = match bind(port).await {
            Ok(socket) => socket,
            Err(err) => {
                // FR-1.7 / ERR_PORT_UNAVAILABLE — surface it and keep retrying
                // rather than dying silently.
                //
                // `continue` rather than `return`: the port may be held by a
                // program the user is about to close, and an app that gave up
                // permanently would have to be restarted to recover.
                state
                    .emit_error(
                        &app,
                        "discovery",
                        crate::history::Direction::Incoming,
                        "LocalDrop",
                        &err,
                    )
                    .await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        // §6.2 — the port actually bound, which fallback may have moved. It is
        // recorded here so the heartbeat advertises the truth rather than the
        // configured value, and so diagnostics can show it.
        let bound = socket.local_addr().map(|a| a.port()).unwrap_or(port);
        {
            // Scoped so the write lock is released before the loops start
            // competing for it.
            let mut ports = state.bound_ports.write().await;
            ports.0 = bound;
        }
        tracing::info!(port = bound, "discovery bound");

        let socket = Arc::new(socket);
        let cancel = CancellationToken::new();

        let handles = vec![
            tokio::spawn(heartbeat_loop(
                app.clone(),
                state.clone(),
                socket.clone(),
                cancel.clone(),
            )),
            tokio::spawn(receive_loop(
                app.clone(),
                state.clone(),
                socket.clone(),
                cancel.clone(),
            )),
            tokio::spawn(prune_loop(app.clone(), state.clone(), cancel.clone())),
        ];

        // Blocks until the interfaces change underneath us.
        watch_network(app.clone(), state.clone()).await;

        // Signal all three workers at once, then wait for each to actually
        // finish. Awaiting the handles matters: without it the next iteration
        // could bind the same port while the old receive loop still held it.
        cancel.cancel();
        for handle in handles {
            // Errors ignored — a join error means the task panicked, which is
            // already logged, and there is nothing useful to do but rebind.
            let _ = handle.await;
        }
        tracing::info!("rebinding discovery after network change");
    }
}

/// §6.2 — port fallback scans `port..=port+9`.
///
/// Bounded repetition over ten candidate ports, returning the first that both
/// binds *and* accepts the broadcast option. `saturating_add` rather than `+`
/// so a configured port near `u16::MAX` cannot overflow the range.
async fn bind(port: u16) -> TransferResult<UdpSocket> {
    for candidate in port..=port.saturating_add(PORT_FALLBACK_RANGE) {
        match UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, candidate))).await {
            Ok(socket) => {
                // A bound socket that cannot broadcast is useless to us — it
                // could receive heartbeats but never send one, so the device
                // would see peers while remaining invisible to them. Treated as
                // a failed candidate and skipped, not returned.
                if let Err(e) = socket.set_broadcast(true) {
                    tracing::warn!(error = %e, "could not enable broadcast");
                    continue;
                }
                return Ok(socket);
            }
            // Already in use, or refused. Silent because trying the next port
            // is the designed behaviour, not an anomaly worth logging ten times.
            Err(_) => continue,
        }
    }

    // Every candidate failed: something else owns the whole range.
    Err(TransferError::PortUnavailable {
        first: port,
        last: port.saturating_add(PORT_FALLBACK_RANGE),
    })
}

async fn heartbeat_loop(
    app: AppHandle,
    state: Arc<AppState>,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    loop {
        // Beat first, then wait — so the very first heartbeat goes out
        // immediately on launch rather than 3 s later, which is most of §2.3's
        // S1 budget for a peer becoming visible.
        send_heartbeat(&state, &socket).await;

        // Piggy-backs a diagnostics refresh on the heartbeat cadence, so the
        // header's address and port readouts stay current without their own
        // timer.
        let _ = app.emit(events::NETWORK_CHANGED, snapshot(&state).await);

        // Three-way race. Whichever completes first decides what happens next;
        // the other two futures are dropped.
        tokio::select! {
            // Rebind or shutdown: leave the loop entirely.
            _ = cancel.cancelled() => return,
            // FR-1.1 — the ordinary 3 s cadence.
            _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {}
            // FR-1.6 — rescan fires an immediate beat instead of waiting 3 s.
            _ = state.rescan.notified() => {}
        }
    }
}

async fn send_heartbeat(state: &Arc<AppState>, socket: &UdpSocket) {
    // Cloned out under a scoped read lock rather than held across the awaits
    // below: the settings lock is contended by every command, and keeping it
    // while writing to a socket would stall the UI behind the network.
    //
    // Re-read every beat, which is what makes FR-6.1's "propagates on the next
    // heartbeat" true without any notification plumbing.
    let (alias, port) = {
        let settings = state.settings.read().await;
        (settings.device_alias.clone(), settings.discovery_port)
    };
    let tcp_port = state.bound_ports.read().await.1;
    let device_id = state.identity.read().await.device_id.clone();

    let heartbeat = Heartbeat {
        protocol_version: PROTOCOL_VERSION,
        device_id,
        alias,
        os: PeerOs::current(),
        tcp_port,
        client_version: crate::protocol::CLIENT_VERSION.to_string(),
        // §6.2 — advertised so peers can render this card non-droppable while
        // a transfer is in flight, rather than letting a drop fail later.
        state: if state.is_busy() {
            PeerState::Busy
        } else {
            PeerState::Ready
        },
    };

    // Cannot fail for this struct; returning rather than unwrapping keeps a
    // serialization bug from taking down the discovery loop.
    let Ok(bytes) = serde_json::to_vec(&heartbeat) else {
        return;
    };

    // One datagram per interface. §6.2 sends to each directed broadcast address
    // rather than once to 255.255.255.255, because many APs and Android builds
    // silently drop the global address — see `broadcast_targets`.
    //
    // Each send is independent: a failure on a down or restricted interface
    // must not prevent the others. Logged at debug, not warn, because a
    // transient failure here is normal on a machine with virtual adapters.
    for target in broadcast_targets() {
        let addr = SocketAddr::V4(SocketAddrV4::new(target, port));
        if let Err(e) = socket.send_to(&bytes, addr).await {
            tracing::debug!(%addr, error = %e, "heartbeat send failed");
        }
    }
}

async fn receive_loop(
    app: AppHandle,
    state: Arc<AppState>,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    // Allocated once and reused for every datagram, rather than per iteration:
    // this loop runs for the life of the binding and heartbeats arrive
    // continuously from every peer.
    let mut buf = vec![0u8; RECV_BUF];

    // FR-1.5 — remembers which incompatible peers have already been announced.
    // Without it, a peer on an old protocol version would raise a fresh chip
    // every 3 s forever. Scoped to this binding generation, so a rebind gives
    // the user the warning again on the new network.
    let mut announced_incompatible: BTreeSet<String> = BTreeSet::new();

    loop {
        // Cancellation raced against the receive, because `recv_from` blocks
        // indefinitely on a quiet network — without the select, a rebind would
        // hang until the next datagram happened to arrive.
        let received = tokio::select! {
            _ = cancel.cancelled() => return,
            result = socket.recv_from(&mut buf) => result,
        };

        let (len, from) = match received {
            Ok(v) => v,
            Err(e) => {
                // A recv error is usually transient (the interface going down
                // just before the supervisor notices). The short sleep prevents
                // a hot spin if it is persistent; the supervisor rebinds.
                tracing::debug!(error = %e, "discovery recv failed");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };

        // Anything on this port that is not our JSON. Silent by design: on a
        // busy LAN this port receives unrelated broadcast traffic, and logging
        // it would bury the real events.
        let Ok(hb) = serde_json::from_slice::<Heartbeat>(&buf[..len]) else {
            continue; // not ours, or truncated — ignore quietly
        };

        // FR-1.4 — loopback suppression. Our own broadcast comes back to us on
        // the same socket; without this the device would list itself as a peer.
        // Compared by device id rather than by source address, because the
        // address varies per interface while the id does not.
        if hb.device_id == state.identity.read().await.device_id {
            continue;
        }

        // FR-1.5 — incompatible peers surface once, as a chip, not a card.
        // `insert` returns false if already present, so the emit happens only
        // on first sighting — the set is both the memory and the guard.
        if hb.protocol_version != PROTOCOL_VERSION {
            if announced_incompatible.insert(hb.device_id.clone()) {
                let _ = app.emit(
                    events::PEER_INCOMPATIBLE,
                    IncompatiblePeer {
                        device_id: hb.device_id,
                        alias: hb.alias,
                        ip: from.ip().to_string(),
                        client_version: hb.client_version,
                        protocol_version: hb.protocol_version,
                    },
                );
            }
            continue;
        }

        // Selection on what the registry made of the heartbeat. The three arms
        // exist so the UI is only told about changes it can see — the
        // `Unchanged` case is the overwhelmingly common one (a repeat beat from
        // a peer whose alias, IP, state and port are all the same), and
        // emitting for it would repaint the radar every 3 s per peer.
        match state.registry.lock().await.upsert(hb, from.ip()) {
            Upsert::Added(peer) => {
                // Logged at info: a new peer is a genuine, infrequent event.
                tracing::info!(alias = peer.alias, ip = peer.ip, "peer discovered");
                let _ = app.emit(events::PEER_DISCOVERED, peer);
            }
            Upsert::Updated(peer) => {
                // Not logged — a rename or a Ready→Busy flip is routine.
                let _ = app.emit(events::PEER_UPDATED, peer);
            }
            Upsert::Unchanged => {}
        }
    }
}

async fn prune_loop(app: AppHandle, state: Arc<AppState>, cancel: CancellationToken) {
    loop {
        // Sleep first, so nothing is pruned in the instant after a rebind when
        // no peer has had a chance to beat yet.
        //
        // Polled at 2 s against a 12 s TTL: frequent enough that a departed
        // peer disappears promptly, infrequent enough to stay within §2.3's
        // 1%-of-a-core idle budget (S7).
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(PRUNE_INTERVAL) => {}
        }

        // The lock is released before the loop below — `prune` returns owned
        // ids precisely so the emits do not happen while holding it.
        let expired = state.registry.lock().await.prune();

        // One event per departed peer, so the UI can animate each card out
        // individually (FR-1.3) rather than rebuilding the grid.
        for device_id in expired {
            tracing::info!(device_id, "peer pruned");
            let _ = app.emit(events::PEER_LOST, device_id);
        }
    }
}

/// FR-1.8 — returns once the set of usable interfaces changes.
async fn watch_network(app: AppHandle, state: Arc<AppState>) {
    // Baseline captured once, before the loop: every later comparison is
    // against the state at bind time, not against the previous poll. That way a
    // change followed by a revert still counts as a change worth rebinding for.
    let fingerprint = interface_fingerprint();

    // Polling rather than subscribing to an OS interface-change notification,
    // which has no portable API across Windows and Android. 3 s comfortably
    // meets FR-1.8's 5 s rebind requirement.
    loop {
        tokio::time::sleep(NETWORK_POLL).await;
        // Returning hands control back to the supervisor, which rebinds the
        // sockets and calls us again with a fresh baseline.
        if interface_fingerprint() != fingerprint {
            let _ = app.emit(events::NETWORK_CHANGED, snapshot(&state).await);
            return;
        }
    }
}

/// A comparable summary of the current interfaces.
///
/// Name *and* address, because either changing matters: a new adapter appearing
/// and an existing adapter being handed a different DHCP lease both invalidate
/// the bound sockets. A `BTreeSet` so the comparison is order-independent — the
/// OS does not enumerate interfaces in a stable order, and an ordering change
/// alone must not trigger a spurious rebind.
fn interface_fingerprint() -> BTreeSet<String> {
    usable_interfaces()
        .into_iter()
        .map(|i| format!("{}:{}", i.name, i.ip))
        .collect()
}

/// A usable IPv4 interface and its derived broadcast address.
pub struct Interface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub broadcast: Ipv4Addr,
}

/// IPv4, up, non-loopback, non-link-local interfaces.
///
/// Link-local (169.254/16) is excluded because an APIPA address means DHCP
/// failed — broadcasting there just produces phantom peers.
pub fn usable_interfaces() -> Vec<Interface> {
    // An empty vec, not an error: callers read this as "offline", which drives
    // FR-1.7's empty state. There is no useful distinction between "no
    // interfaces" and "could not enumerate interfaces" from the UI's side.
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };

    // One filtering pass. `filter_map` rather than `filter` then `map`, because
    // each entry is simultaneously being tested and converted.
    addrs
        .into_iter()
        .filter_map(|iface| {
            let name = iface.name.clone();
            match iface.addr {
                if_addrs::IfAddr::V4(v4) => {
                    // Three exclusions, each for a different reason:
                    //   - loopback (127/8): reaches nothing but this machine.
                    //   - link-local (169.254/16): an APIPA address means DHCP
                    //     failed, and broadcasting there produces phantom peers.
                    //   - unspecified (0.0.0.0): not a real address.
                    if v4.ip.is_loopback() || v4.ip.is_link_local() || v4.ip.is_unspecified() {
                        return None;
                    }

                    let netmask = v4.netmask;

                    // Prefer the OS-reported broadcast address; compute it only
                    // when absent. Some adapters — notably virtual ones on
                    // Windows — report a netmask but no broadcast address.
                    let broadcast = v4
                        .broadcast
                        .unwrap_or_else(|| derive_broadcast(v4.ip, netmask));

                    Some(Interface {
                        name,
                        ip: v4.ip,
                        netmask,
                        broadcast,
                    })
                }
                // IPv6 is out of scope: §6.2's discovery is IPv4 broadcast, and
                // IPv6 has no broadcast address at all (it uses multicast).
                if_addrs::IfAddr::V6(_) => None,
            }
        })
        .collect()
}

/// `ip | !netmask` — the directed broadcast address for the subnet.
///
/// Setting every host bit: for 192.168.1.44/24 this yields 192.168.1.255. Done
/// on the `u32` representation because the operation is bitwise over the whole
/// address, not per octet.
fn derive_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    let ip = u32::from(ip);
    let mask = u32::from(netmask);
    Ipv4Addr::from(ip | !mask)
}

/// §6.2 — prefer per-interface directed broadcast; fall back to the global
/// address only when no interface reports one, because many Wi-Fi APs and
/// Android builds silently drop 255.255.255.255.
pub fn broadcast_targets() -> Vec<Ipv4Addr> {
    let mut targets: Vec<Ipv4Addr> = usable_interfaces()
        .into_iter()
        .map(|i| i.broadcast)
        .collect();

    // Sort then dedup, in that order — `dedup` only removes *consecutive*
    // duplicates, so without the sort it would miss repeats. Two adapters on
    // the same subnet share a broadcast address, and sending the same heartbeat
    // to it twice would make this device appear twice in some peers' logs.
    targets.sort();
    targets.dedup();

    // Last resort. Kept as a fallback rather than a default because many Wi-Fi
    // APs and Android builds silently drop 255.255.255.255 — sending there when
    // a directed address exists would be strictly worse.
    if targets.is_empty() {
        targets.push(Ipv4Addr::BROADCAST);
    }
    targets
}

/// Pick the interface to show the user as "this device's address".
///
/// Interface order from the OS is arbitrary, and machines running VMware,
/// VirtualBox, Hyper-V, WSL or Docker typically have several virtual adapters
/// with routable-looking addresses (192.168.x.1). Taking the first one makes the
/// UI report, say, 192.168.118.1 for a VMware adapter while the real LAN address
/// is 192.168.100.44 — which reads as a bug to the user and makes the subnet
/// shown on the radar wrong.
///
/// `local_ip()` resolves the address of the interface that actually carries
/// outbound traffic, so prefer whichever interface owns it. Discovery itself is
/// unaffected either way: heartbeats go to every interface's broadcast address.
fn primary_interface(interfaces: &[Interface]) -> Option<&Interface> {
    // The pattern deliberately matches only `Ok(V4)`: an error, or an IPv6
    // answer, both fall through to the positional fallback rather than being
    // handled separately, because neither can be matched against this list.
    if let Ok(IpAddr::V4(preferred)) = local_ip_address::local_ip() {
        // The routable address may belong to an interface this function
        // filtered out, so a match is not guaranteed even when the lookup
        // succeeds — hence the search rather than a direct construction.
        if let Some(matched) = interfaces.iter().find(|i| i.ip == preferred) {
            return Some(matched);
        }
    }

    // Arbitrary but deterministic. `None` when the list is empty, which the
    // caller renders as offline.
    interfaces.first()
}

/// Capture the current network state for the UI.
pub async fn snapshot(state: &Arc<AppState>) -> NetworkSnapshot {
    let interfaces = usable_interfaces();
    let ports = *state.bound_ports.read().await;
    let primary = primary_interface(&interfaces);

    NetworkSnapshot {
        // FR-1.7 — "online" means at least one usable interface, not internet
        // reachability. LocalDrop needs a LAN, and nothing beyond it.
        online: !interfaces.is_empty(),
        interface: primary.map(|i| i.name.clone()),
        // Falls back to the routable address even when no interface qualified,
        // so the diagnostics panel can still show something useful on a machine
        // whose only address this module filtered out.
        local_ip: primary.map(|i| i.ip.to_string()).or_else(|| {
            local_ip_address::local_ip()
                .ok()
                .map(|ip: IpAddr| ip.to_string())
        }),
        netmask: primary.map(|i| i.netmask.to_string()),
        broadcast_targets: broadcast_targets().iter().map(|b| b.to_string()).collect(),
        discovery_port: ports.0,
        transfer_port: ports.1,
    }
}
