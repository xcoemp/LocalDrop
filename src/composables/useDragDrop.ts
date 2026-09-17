/**
 * useDragDrop.ts — global OS drag-and-drop (PRD UI-1, FR-2.1).
 *
 * The whole window is a drop target. Hovering a peer card designates it the
 * destination; dropping on empty canvas opens the peer picker instead of
 * guessing. Desktop only — Tauri's Android webview emits no drag events.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */

import { onBeforeUnmount, onMounted, ref } from "vue";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { usePeerStore } from "@/stores/usePeerStore";
import { useTransferStore } from "@/stores/useTransferStore";

export function useDragDrop() {
  const peers = usePeerStore();
  const transfers = useTransferStore();

  const dragging = ref(false);
  const hoverPeerId = ref<string | null>(null);
  /** Paths waiting for the user to pick a destination. */
  const pendingPaths = ref<string[]>([]);

  let unlisten: UnlistenFn | null = null;

  /**
   * Which peer card, if any, sits under a pointer position.
   *
   * Tauri reports physical pixels; `elementFromPoint` wants CSS pixels.
   */
  function peerAt(x: number, y: number): string | null {
    // Guard against a zero/undefined ratio: dividing by it would yield NaN
    // coordinates and `elementFromPoint` would silently return null, making
    // every drop look like it landed on empty canvas.
    const ratio = window.devicePixelRatio || 1;
    const element = document.elementFromPoint(x / ratio, y / ratio);

    // `closest` walks up from whatever leaf node is topmost — usually a label
    // or an icon inside the card — to the card element that carries the id.
    const card = element?.closest<HTMLElement>("[data-peer-id]");

    // Absent either the element or the ancestor card, the pointer is over
    // background; `?? null` collapses both cases to the same answer.
    return card?.dataset.peerId ?? null;
  }

  async function dispatch(paths: string[], deviceId: string) {
    try {
      await transfers.sendFiles(deviceId, paths);
    } catch (error) {
      // Rust hands back a typed ErrorPayload; anything else is a genuine bug.
      // The cast is narrowed with `??` below rather than trusted, so a thrown
      // string or a JS TypeError still produces a usable toast (FR-5.6).
      const payload = error as { message?: string; code?: string };
      transfers.toast({
        kind: "error",
        title: payload.message ?? "Couldn't start the transfer.",
        code: payload.code,
      });
    }
  }

  /**
   * Resolve a completed drop to a destination, asking the user only when the
   * answer is genuinely ambiguous.
   *
   * The three branches below are ordered cheapest-and-most-certain first.
   */
  async function onDrop(paths: string[], target: string | null) {
    // The OS can report a drop with no paths (dragging selected text, an image
    // from a browser). There is nothing to send, and prompting for a peer would
    // be a dead end.
    if (paths.length === 0) return;

    // Prefer the card actually hovered. Falling back to the sole peer when
    // there is exactly one is the case UI-1's "ask, don't assume" rule does not
    // need to cover — with one candidate there is nothing to disambiguate.
    const deviceId = target ?? (peers.list.length === 1 ? peers.list[0].deviceId : null);

    // Destination known: select it so the UI agrees with what is happening, and
    // send immediately. This is the two-action golden path of §3.1.
    if (deviceId) {
      peers.select(deviceId);
      await dispatch(paths, deviceId);
      return;
    }

    // No peers at all. A picker with an empty list explains nothing, so say
    // what is actually wrong instead.
    if (peers.list.length === 0) {
      transfers.toast({
        kind: "info",
        title: "No peers on this network yet",
        body: "Open LocalDrop on another device to see it here.",
      });
      return;
    }

    // UI-1 — ambiguous drop (two or more peers, none hovered), so ask rather
    // than assume. Parking the paths here is what opens the picker dialog.
    pendingPaths.value = paths;
  }

  /** Called by the picker once the user has chosen among several peers. */
  function resolvePending(deviceId: string) {
    // Snapshot and clear before dispatching: `dispatch` is async, and leaving
    // the paths in place would keep the picker open behind the transfer.
    const paths = pendingPaths.value;
    pendingPaths.value = [];
    peers.select(deviceId);
    void dispatch(paths, deviceId);
  }

  function cancelPending() {
    pendingPaths.value = [];
  }

  onMounted(async () => {
    try {
      unlisten = await getCurrentWebview().onDragDropEvent((event) => {
        const payload = event.payload;

        // Selection over the event's discriminated union. Ordered by frequency:
        // "over" fires continuously throughout a drag, the others fire once.
        if (payload.type === "over") {
          dragging.value = true;
          // Recomputed on every move so the highlight tracks the cursor between
          // cards rather than latching onto the first one entered.
          hoverPeerId.value = peerAt(payload.position.x, payload.position.y);
          return;
        }

        if (payload.type === "drop") {
          // Read the hover target *before* resetting it — the reset below must
          // happen synchronously so the overlay disappears immediately, but
          // `onDrop` still needs to know where the files landed.
          const target = hoverPeerId.value;
          dragging.value = false;
          hoverPeerId.value = null;
          void onDrop(payload.paths, target);
          return;
        }

        // Remaining case ("leave", plus any variant a future Tauri adds):
        // the drag is over and nothing was dropped, so clear the affordance.
        // Handled as a fallthrough rather than an explicit `=== "leave"` so a
        // new event type cannot leave the overlay stuck on screen.
        dragging.value = false;
        hoverPeerId.value = null;
      });
    } catch {
      // Mobile webview: no drag events, and no drag affordance is shown. This
      // is an expected platform difference, not an error worth surfacing.
    }
  });

  // Optional call: on mobile the listener was never registered, so there is
  // nothing to tear down.
  onBeforeUnmount(() => unlisten?.());

  return { dragging, hoverPeerId, pendingPaths, resolvePending, cancelPending };
}
