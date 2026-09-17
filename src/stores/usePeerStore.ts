/**
 * usePeerStore.ts — discovered peers and network state (PRD FR-1).
 *
 * The backend owns the peer table; this store is a reactive projection of the
 * `peer://*` event stream, seeded once on mount. Nothing here decides whether a
 * peer exists — that is `registry.rs` with its 12 s TTL — so the store's job is
 * to project, filter, and remember which one the user selected.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

import type { IncompatiblePeer, NetworkSnapshot, Peer } from "@/types/protocol";

export const usePeerStore = defineStore("peers", () => {
  // Keyed by `deviceId` rather than held as an array: heartbeats arrive every
  // 3 s per peer and each one is an upsert, which is O(1) on a Map and a linear
  // scan on an array.
  const peers = ref(new Map<string, Peer>());
  /** FR-1.5 — surfaced as dismissible chips, never as connectable cards. */
  const incompatible = ref(new Map<string, IncompatiblePeer>());
  const network = ref<NetworkSnapshot | null>(null);
  const selectedId = ref<string | null>(null);
  const filter = ref("");
  const scanning = ref(false);

  const list = computed(() => [...peers.value.values()]);

  /** Matches alias, IP, or OS, mirroring the mockup's filter placeholder. */
  const visible = computed(() => {
    const q = filter.value.trim().toLowerCase();

    // An empty query returns the same array reference `list` already computed,
    // so no needless copy and no spurious re-render of the grid.
    if (!q) return list.value;

    // Repetition: one pass over the peers, matching against a single
    // concatenated haystack. Cheap enough to run on every keystroke — the peer
    // count is bounded by the LAN, and §9 budgets 4 peers as the typical case.
    return list.value.filter((p) => `${p.alias} ${p.ip} ${p.os}`.toLowerCase().includes(q));
  });

  const selected = computed(() =>
    // Two conditions collapse into one expression: nothing selected, or a
    // selection that has since been pruned by TTL. Both mean "no destination",
    // and callers must not distinguish them.
    selectedId.value ? (peers.value.get(selectedId.value) ?? null) : null,
  );

  /** FR-1.7 — drives the "No LAN connection" empty state. */
  const online = computed(
    () =>
      // Defaults to `true` before the first snapshot arrives. Optimistic on
      // purpose: briefly showing an empty radar is better than flashing "No LAN
      // connection" during the few milliseconds of startup.
      network.value?.online ?? true,
  );

  function upsert(peer: Peer) {
    // Insert and update are the same operation — a repeat heartbeat carries a
    // possibly-changed alias, state, or IP (§6.6, dual-homed host), and the
    // newest packet always wins.
    peers.value.set(peer.deviceId, peer);
  }

  function remove(deviceId: string) {
    // `"*"` is the wildcard the backend uses to mean "drop everything", emitted
    // on a network change (FR-1.8) when every previously known peer is
    // unreachable by definition.
    if (deviceId === "*") {
      peers.value.clear();
      selectedId.value = null;
      return;
    }

    peers.value.delete(deviceId);

    // Clear the selection only if it was *this* peer. Guarding the assignment
    // matters: unconditionally nulling it would deselect the user's chosen
    // destination every time any unrelated peer timed out.
    if (selectedId.value === deviceId) selectedId.value = null;
  }

  function addIncompatible(peer: IncompatiblePeer) {
    // Also keyed by id, so a peer broadcasting an unsupported version every
    // 3 s produces one chip rather than a growing pile (FR-1.5).
    incompatible.value.set(peer.deviceId, peer);
  }

  function dismissIncompatible(deviceId: string) {
    incompatible.value.delete(deviceId);
  }

  function select(deviceId: string | null) {
    selectedId.value = deviceId;
  }

  function setNetwork(snapshot: NetworkSnapshot) {
    network.value = snapshot;
  }

  async function refresh() {
    // Issued in parallel: they are independent reads, and awaiting them in
    // sequence would double the delay before the radar first paints (§9's
    // cold-start budget).
    const [found, net] = await Promise.all([
      invoke<Peer[]>("get_peers"),
      invoke<NetworkSnapshot>("get_network"),
    ]);

    // Rebuild rather than merge. The backend's table is authoritative, so a
    // wholesale replacement also removes anything this store still held that
    // the backend has since pruned.
    peers.value = new Map(found.map((p) => [p.deviceId, p]));
    network.value = net;
  }

  /** FR-1.6 — clears the table and forces an immediate heartbeat. */
  async function rescan() {
    scanning.value = true;
    peers.value.clear();
    selectedId.value = null;
    try {
      await invoke("rescan");
      // Peers reappear via events; the spinner runs for one heartbeat window so
      // the action reads as deliberate rather than instantaneous.
      await new Promise((resolve) => setTimeout(resolve, 1200));
    } finally {
      // `finally` so a failed `invoke` cannot leave the spinner running
      // forever, which would look like a hang rather than an error.
      scanning.value = false;
    }
  }

  return {
    peers,
    incompatible,
    network,
    selectedId,
    filter,
    scanning,
    list,
    visible,
    selected,
    online,
    upsert,
    remove,
    addIncompatible,
    dismissIncompatible,
    select,
    setNetwork,
    refresh,
    rescan,
  };
});
