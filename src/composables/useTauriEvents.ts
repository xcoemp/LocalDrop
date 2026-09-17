/**
 * useTauriEvents.ts — the typed Rust → frontend event bridge (PRD §6.1).
 *
 * Mounted once from App.vue. Everything that reacts to a backend event lives
 * here, so there is exactly one place to look when asking "what happens when a
 * transfer finishes?".
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */

import { onBeforeUnmount, onMounted } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

import { usePeerStore } from "@/stores/usePeerStore";
import { useSettingsStore } from "@/stores/useSettingsStore";
import { useTransferStore } from "@/stores/useTransferStore";
import { formatBytes } from "@/composables/useFormat";
import {
  EVENTS,
  type HistoryEntry,
  type IncompatiblePeer,
  type NetworkSnapshot,
  type OfferPayload,
  type Peer,
  type Settings,
  type TransferErrorPayload,
  type TransferSnapshot,
} from "@/types/protocol";

/** AND-3 — notifications degrade gracefully if the user declines. */
let notificationsAllowed = false;

async function ensureNotificationPermission() {
  try {
    notificationsAllowed = await isPermissionGranted();

    // Only prompt when not already granted. Asking again on every launch would
    // be both pointless and, on Android, permanently denied after two refusals.
    if (!notificationsAllowed) {
      notificationsAllowed = (await requestPermission()) === "granted";
    }
  } catch {
    // AND-3 — a platform without the plugin, or a user who declined, must not
    // break the app. Transfers work fine; only the notifications are absent.
    notificationsAllowed = false;
  }
}

function notify(title: string, body: string) {
  // Checked here rather than at each call site, so no caller can forget it.
  if (!notificationsAllowed) return;
  try {
    sendNotification({ title, body });
  } catch {
    /* notifications are a nicety, never a failure path */
  }
}

/**
 * FR-6.3 — completion chime. Synthesized rather than bundled so the app ships
 * without an audio asset and stays under the installer budget (§9).
 */
function chime() {
  try {
    const ctx = new AudioContext();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();

    // A short rising sine: 880 Hz → 1320 Hz (A5 to E6).
    osc.type = "sine";
    osc.frequency.setValueAtTime(880, ctx.currentTime);
    osc.frequency.exponentialRampToValueAtTime(1320, ctx.currentTime + 0.08);

    // Enveloped rather than switched on: an instantaneous gain step produces an
    // audible click. Ramps start from 0.0001 instead of 0 because
    // `exponentialRampToValueAtTime` cannot approach or leave exact zero.
    gain.gain.setValueAtTime(0.0001, ctx.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.12, ctx.currentTime + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + 0.35);

    osc.connect(gain).connect(ctx.destination);
    osc.start();
    osc.stop(ctx.currentTime + 0.36);
    // Closing the context releases the audio device. Without this, one context
    // leaks per completed transfer and browsers cap how many may exist.
    osc.onended = () => void ctx.close();
  } catch {
    /* audio is optional */
  }
}

export function useTauriEvents() {
  const peers = usePeerStore();
  const transfers = useTransferStore();
  const settings = useSettingsStore();

  // Collected so every listener can be detached together on unmount. An array
  // rather than named variables because the count changes as events are added,
  // and the teardown loop should not have to be edited in step.
  const unlisten: UnlistenFn[] = [];

  /**
   * Everything that happens when a transfer reaches a terminal state.
   *
   * The guards below are ordered from broadest to narrowest so that each
   * subsequent branch can assume the ones above it held.
   */
  async function onComplete(entry: HistoryEntry) {
    // History records every outcome, including failures and cancellations —
    // FR-5.4's list is a log, not a success list.
    transfers.addHistory(entry);

    // Everything past this point is success-only celebration: no chime for a
    // failed transfer, no "received" notification for a rejected one.
    if (entry.outcome !== "completed") return;

    // Selection on direction: the receiver has a payload to act on, the sender
    // only needs an acknowledgement.
    if (entry.direction === "incoming") {
      // FR-3.3 — auto-copy a received snippet, and say so. Both conditions are
      // required: a file transfer has no `text`, and the user may have turned
      // the setting off.
      if (entry.text !== null && settings.settings?.autoCopySnippets) {
        try {
          await writeText(entry.text);
          transfers.toast({
            kind: "success",
            title: "Snippet copied to clipboard",
            // Truncated: a 1 MB snippet would otherwise be pasted into a toast.
            body: entry.text.slice(0, 80),
          });
        } catch {
          // The clipboard can be locked by another process on Windows. The
          // snippet still arrived, so report the weaker claim rather than an
          // error — it remains copyable by hand from history.
          transfers.toast({ kind: "info", title: "Snippet received" });
        }
      }

      // FR-4.1 — the receiver is told, whether or not the window is focused.
      notify(
        `Received from ${entry.peerAlias}`,
        `${entry.label} · ${formatBytes(entry.totalBytes)}`,
      );
    } else {
      notify(`Sent to ${entry.peerAlias}`, entry.label);
    }

    // FR-6.3 — opt-in, and checked after the outcome guard so a failure is
    // never announced with a success chime.
    if (settings.settings?.completionSound) chime();

    if (entry.skipped.length > 0) {
      // FR-2.3 — skipped items are reported rather than silently dropped.
      // Pluralised inline, and capped at three names so a folder of symlinks
      // cannot produce an unbounded toast.
      transfers.toast({
        kind: "info",
        title: `${entry.skipped.length} item${entry.skipped.length === 1 ? "" : "s"} skipped`,
        body: entry.skipped.slice(0, 3).join(", "),
      });
    }
  }

  onMounted(async () => {
    // Requested before the listeners attach, so the first arriving offer can
    // already raise a notification (FR-4.1).
    await ensureNotificationPermission();

    unlisten.push(
      // Discovery. `discovered` and `updated` share a handler because `upsert`
      // is idempotent — the distinction matters to the backend's registry, not
      // to a Map keyed by device id.
      await listen<Peer>(EVENTS.peerDiscovered, (e) => peers.upsert(e.payload)),
      await listen<Peer>(EVENTS.peerUpdated, (e) => peers.upsert(e.payload)),
      await listen<string>(EVENTS.peerLost, (e) => peers.remove(e.payload)),
      await listen<IncompatiblePeer>(EVENTS.peerIncompatible, (e) =>
        peers.addIncompatible(e.payload),
      ),
      await listen<NetworkSnapshot>(EVENTS.networkChanged, (e) => peers.setNetwork(e.payload)),

      await listen<OfferPayload>(EVENTS.transferOffer, (e) => {
        transfers.addOffer(e.payload);
        // Selection on payload kind: a snippet has no meaningful item count or
        // size to quote, so it gets its own wording.
        notify(
          `${e.payload.peerAlias} wants to send you something`,
          e.payload.isText
            ? "A text snippet"
            : `${e.payload.itemCount} item${e.payload.itemCount === 1 ? "" : "s"} · ${formatBytes(e.payload.totalBytes)}`,
        );
      }),

      // Arrives at ~10 Hz per active transfer (FR-5.2), so the handler stays a
      // single Map write with no derived work.
      await listen<TransferSnapshot>(EVENTS.transferProgress, (e) =>
        transfers.applyProgress(e.payload),
      ),

      await listen<HistoryEntry | string>(EVENTS.transferComplete, (e) => {
        // A rejected offer emits just its id; a finished transfer emits the
        // full history entry. Discriminated by runtime type because the two
        // cases share one event name on the Rust side.
        if (typeof e.payload === "string") {
          transfers.dropOffer(e.payload);
          return;
        }
        void onComplete(e.payload);
      }),

      await listen<TransferErrorPayload>(EVENTS.transferError, (e) =>
        transfers.applyError(e.payload),
      ),

      // Rust is the owner of settings, so a change made anywhere — including
      // normalization it applied itself — flows back through here.
      await listen<Settings>(EVENTS.settingsChanged, (e) => settings.apply(e.payload)),
    );
  });

  onBeforeUnmount(() => {
    // Repetition: detach every listener. Skipping this would leave the previous
    // handlers bound to a disposed component after a hot reload, so each event
    // would be handled twice.
    unlisten.forEach((fn) => fn());
  });
}
