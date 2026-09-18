/**
 * useTransferStore.ts — active transfers, pending offers, history, and toasts
 * (PRD FR-5, FR-4).
 *
 * Like the peer store, this is a projection of backend state rather than the
 * source of truth: Rust owns the queue and the history file, and every mutation
 * here is either an event applied or a command dispatched.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

import type {
  HistoryEntry,
  OfferPayload,
  SendTextResult,
  TransferErrorPayload,
  TransferSnapshot,
} from "@/types/protocol";

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  title: string;
  body?: string;
  /**
   * FR-5.6 — the error code. Never rendered as visible text: `ERR_WRITE_FAILED`
   * means nothing to the person reading it, and testers reported it as the app
   * "showing debug output". It is carried here so the copy-details action can
   * include it, and so support still has it.
   */
  code?: string;
  /**
   * The raw technical cause — an OS error string such as "An existing
   * connection was forcibly closed by the remote host (os error 10054)".
   *
   * Also never rendered. It used to be passed as `body`, which put exactly that
   * text in front of users; `title` already says the same thing in English.
   */
  detail?: string;
}

/**
 * The phases from §6.4's state machine that no further progress can follow.
 * A Set rather than a chain of `||` comparisons: membership is asked on every
 * progress event, and adding a phase should not mean editing a condition.
 */
const TERMINAL = new Set(["completed", "cancelled", "failed", "rejected"]);
/** How long a finished row lingers in Active before it is history-only. */
const LINGER_MS = 4000;

export const useTransferStore = defineStore("transfers", () => {
  const active = ref(new Map<string, TransferSnapshot>());
  const offers = ref<OfferPayload[]>([]);
  const history = ref<HistoryEntry[]>([]);
  const toasts = ref<Toast[]>([]);

  let toastSeq = 0;

  const activeList = computed(() =>
    // Sorted by id so row order is stable. Without a deterministic sort, Map
    // iteration order would shuffle as entries are deleted after LINGER_MS and
    // rows would visibly jump while the user is reading them (UI-3's spirit).
    [...active.value.values()].sort((a, b) => a.id.localeCompare(b.id)),
  );

  // Excludes rows still lingering on screen after finishing, so aggregate
  // telemetry and the per-card indicators only reflect live streams.
  const running = computed(() => activeList.value.filter((t) => !TERMINAL.has(t.phase)));

  /** FR-5.3 — aggregate throughput across every live stream. */
  const aggregateRate = computed(() =>
    // Repetition: a fold over the running transfers. Summing committed-byte
    // rates is valid precisely because each is measured the same way (FR-5.2).
    running.value.reduce((sum, t) => sum + t.rateBps, 0),
  );

  // Offers queue, and only the head is prompted for. Two dialogs at once would
  // stack on top of each other and the 30 s deadline (FR-4.2) would run on the
  // hidden one.
  const pendingOffer = computed(() => offers.value[0] ?? null);

  /** Drives the per-card mini progress bar in the peer grid. */
  function forPeer(deviceId: string): TransferSnapshot | null {
    // `find`, not `filter`: FR-2.6 serializes transfers per peer, so there is at
    // most one live stream per device and the first match is the only match.
    return running.value.find((t) => t.peerDeviceId === deviceId) ?? null;
  }

  function applyProgress(snapshot: TransferSnapshot) {
    // Whole-snapshot replacement rather than field merging — Rust sends a
    // complete picture each tick, so there is no partial state to preserve.
    active.value.set(snapshot.id, snapshot);

    if (TERMINAL.has(snapshot.phase)) {
      // Leave the finished row on screen briefly so the user sees the outcome,
      // then let history take over.
      setTimeout(() => active.value.delete(snapshot.id), LINGER_MS);
    }
  }

  function addOffer(offer: OfferPayload) {
    offers.value.push(offer);
  }

  function dropOffer(id: string) {
    offers.value = offers.value.filter((o) => o.id !== id);
  }

  function addHistory(entry: HistoryEntry) {
    // Newest first, and the filter de-duplicates by id so re-emitting an entry
    // updates it in place instead of adding a second copy. Reassignment (rather
    // than `unshift`) keeps the ref's identity change explicit for reactivity.
    history.value = [entry, ...history.value.filter((h) => h.id !== entry.id)];
  }

  function toast(toast: Omit<Toast, "id">, ttl = 5000) {
    // Monotonic local counter: ids only need to be unique within this session,
    // and a counter cannot collide the way `Date.now()` can for two toasts
    // raised in the same millisecond.
    const id = ++toastSeq;
    toasts.value.push({ ...toast, id });
    setTimeout(() => dismissToast(id), ttl);
    return id;
  }

  function dismissToast(id: number) {
    // Filtering by id rather than shifting the array, because the user may have
    // dismissed a toast by hand before its timer fired.
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }

  function applyError(payload: TransferErrorPayload) {
    // FR-5.6 — the human-readable cause is the only thing shown. A longer 8 s
    // TTL than a normal toast: an error is worth reading twice.
    //
    // `detail` is deliberately NOT passed as `body`. It used to be, which meant
    // a dropped connection rendered the raw OS string "An existing connection
    // was forcibly closed by the remote host (os error 10054)" underneath a
    // title that already said "Connection to <device> dropped mid-transfer."
    // The first line is the answer; the second was noise that read as a crash.
    // Both code and detail travel on the toast for the copy action instead.
    toast(
      {
        kind: "error",
        title: payload.message,
        code: payload.code,
        // Normalised from null to undefined so the optional property is simply
        // absent rather than present-and-empty.
        detail: payload.detail ?? undefined,
      },
      8000,
    );
  }

  // -- commands ------------------------------------------------------------

  async function hydrate() {
    // Parallel, and both are needed before the Transfers view can render
    // without flashing an empty state at a user who has history.
    const [live, past] = await Promise.all([
      invoke<TransferSnapshot[]>("get_active_transfers"),
      invoke<HistoryEntry[]>("get_history"),
    ]);
    active.value = new Map(live.map((t) => [t.id, t]));
    history.value = past;
  }

  async function sendFiles(deviceId: string, paths: string[]) {
    // Deliberately not wrapped in try/catch: callers need the rejection to
    // decide what to say, and `useDragDrop`/`SendView` both do exactly that.
    return invoke<string>("send_files", { deviceId, paths });
  }

  async function sendText(deviceId: string, text: string) {
    const result = await invoke<SendTextResult>("send_text", { deviceId, text });

    // Rust decides whether the snippet exceeded 1 MB, because it owns the limit.
    if (result.converted) {
      // FR-3.4 — the substitution is stated rather than done silently.
      toast({
        kind: "info",
        title: "Snippet sent as a file",
        body: "Text over 1 MB is delivered as a .txt file instead.",
      });
    }
    return result;
  }

  /** FR-4.1 — the user's answer to an incoming offer. */
  async function respond(id: string, accept: boolean) {
    // Dropped from the queue *before* awaiting, so the dialog closes on the
    // click rather than after the IPC round-trip, and the next queued offer
    // (if any) can take its place immediately.
    dropOffer(id);
    await invoke("respond_to_offer", { id, accept });
  }

  async function cancel(id: string) {
    // No local state change: the backend answers with a progress event carrying
    // the `cancelled` phase, which `applyProgress` then handles like any other.
    await invoke("cancel_transfer", { id });
  }

  async function clearHistory() {
    // Cleared locally only after Rust confirms, so a failed write cannot leave
    // the UI claiming the history is gone when the file still holds it.
    await invoke("clear_history");
    history.value = [];
  }

  return {
    active,
    offers,
    history,
    toasts,
    activeList,
    running,
    aggregateRate,
    pendingOffer,
    forPeer,
    applyProgress,
    addOffer,
    dropOffer,
    addHistory,
    toast,
    dismissToast,
    applyError,
    hydrate,
    sendFiles,
    sendText,
    respond,
    cancel,
    clearHistory,
  };
});
