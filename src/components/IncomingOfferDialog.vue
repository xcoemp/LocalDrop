<!--
  IncomingOfferDialog.vue — the accept/decline prompt for an inbound transfer,
  with the visible 30 s countdown (FR-4.1, FR-4.2).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { Check, FileText, Files, X } from "lucide-vue-next";

import { formatBytes } from "@/composables/useFormat";
import { useTransferStore } from "@/stores/useTransferStore";

/**
 * FR-4.1 / FR-4.2 — the accept prompt, with the same 30 s deadline the backend
 * enforces. The countdown is shown so the deadline is not a surprise.
 */
const OFFER_TIMEOUT_S = 30;

const transfers = useTransferStore();
// Only the head of the queue is prompted for; the store handles the ordering.
const offer = computed(() => transfers.pendingOffer);
const remaining = ref(OFFER_TIMEOUT_S);

let timer: number | undefined;

/**
 * Restart the countdown whenever the prompted offer changes.
 *
 * Watching the offer rather than mounting a timer once is what makes a queue of
 * offers work: accepting the first one advances `pendingOffer` to the second,
 * and that second offer gets its own full 30 s rather than inheriting whatever
 * was left of the first one's.
 */
watch(
  offer,
  (value) => {
    // Cleared unconditionally and first. Every path below either starts a new
    // interval or wants none, and an orphaned timer would keep decrementing a
    // counter for an offer that is already resolved.
    window.clearInterval(timer);

    // No offer: the dialog is closed, so there is nothing to count down.
    if (!value) return;

    remaining.value = OFFER_TIMEOUT_S;

    // Repetition: one tick per second until the deadline. A 1 s interval rather
    // than a single 30 s timeout because the remaining figure is displayed —
    // FR-4.2's deadline is only fair if the user can see it approaching.
    timer = window.setInterval(() => {
      remaining.value -= 1;
      if (remaining.value <= 0) {
        window.clearInterval(timer);
        // The backend rejects on its own timer; this only clears the dialog.
        // Deliberately not calling `respond(false)` — that would send a second,
        // racing decline for an offer Rust has already timed out.
        transfers.dropOffer(value.id);
      }
    }, 1000);
  },
  // `immediate` so an offer that arrived before this component mounted (during
  // startup hydration) still gets a countdown.
  { immediate: true },
);

// Without this, a hot reload or unmount would leave the interval running and
// mutating a ref nothing renders.
onBeforeUnmount(() => window.clearInterval(timer));
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-150"
    leave-active-class="transition-opacity duration-150"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <!--
      `role="alertdialog"` rather than `dialog`: this interrupts the user with
      something requiring a decision on a deadline.

      Note the absence of a backdrop-click or Esc handler, unlike the other
      dialogs in the app. Dismissing by accident would decline a transfer, so
      both answers must be deliberate button presses.
    -->
    <div
      v-if="offer"
      class="fixed inset-0 z-60 flex items-center justify-center bg-[rgba(9,13,22,0.75)] p-space-md backdrop-blur-[8px]"
      role="alertdialog"
      aria-modal="true"
      :aria-label="`Incoming transfer from ${offer.peerAlias}`"
    >
      <div
        class="flex w-full max-w-md flex-col gap-space-md rounded-xl border border-white/15 bg-surface-container p-space-lg"
      >
        <div class="flex items-start gap-space-md">
          <div
            class="flex h-11 w-11 shrink-0 items-center justify-center rounded-md bg-primary-container text-on-primary-container"
          >
            <!-- Selection on payload kind: a document icon for a snippet, a
                 stack for files. -->
            <component :is="offer.isText ? FileText : Files" :size="22" />
          </div>
          <div class="flex min-w-0 flex-col">
            <h2 class="font-headline-md text-headline-md text-on-surface">
              {{ offer.peerAlias }} wants to send you
              <!--
                FR-4.1 requires the sender, item count, and size. A snippet has
                no meaningful count (it is always one body), so it gets its own
                wording rather than "1 item"; files are pluralised inline.
              -->
              {{
                offer.isText
                  ? "a text snippet"
                  : `${offer.itemCount} item${offer.itemCount === 1 ? "" : "s"}`
              }}
            </h2>
            <span class="telemetry text-on-surface-variant">
              {{ formatBytes(offer.totalBytes) }}
            </span>
          </div>
        </div>

        <!--
          Manifest preview. Rendered only when there is something to list — a
          snippet offer carries no names, and an empty bordered box would look
          like a loading failure.
        -->
        <!--
          `max-h` + `overflow-y-auto` because the names below wrap. This sheet
          has no scroll container of its own, so a preview of long filenames on
          a phone would make it taller than the viewport and carry Accept and
          Decline off the bottom — leaving the user unable to answer an offer
          that expires in 30 s (FR-4.2). Bounding the list keeps the decision
          reachable whatever the names look like.
        -->
        <ul
          v-if="offer.preview.length"
          class="flex max-h-48 flex-col gap-1 overflow-y-auto rounded-md bg-surface-container-lowest p-space-sm"
        >
          <!--
            Repetition over the truncated preview Rust sent, not the whole
            manifest: SEC-2 caps a manifest at 100,000 entries, and rendering
            even a fraction of that into a modal would hang the webview.

            Each name wraps rather than being clipped: this list is the only
            thing the user has to judge an incoming transfer by before accepting
            it, so a name whose tail is cut off defeats the point of showing it.
          -->
          <li
            v-for="name in offer.preview"
            :key="name"
            class="wrap-anywhere font-body-sm text-body-sm text-on-surface-variant"
          >
            {{ name }}
          </li>
          <!--
            Accounts for the difference, so the list never silently understates
            what is about to arrive. Suppressed when the preview is complete.
          -->
          <li
            v-if="offer.itemCount > offer.preview.length"
            class="font-body-sm text-body-sm text-outline"
          >
            + {{ offer.itemCount - offer.preview.length }} more
          </li>
        </ul>

        <!-- §8 — the trust model is stated, not implied. -->
        <p class="font-body-sm text-body-sm text-outline">
          Transfers are unencrypted. Only accept from devices you trust on this network.
        </p>

        <div class="flex items-center justify-between gap-space-md">
          <!--
            Clamped at zero: the interval can fire once more between reaching 0
            and the dialog unmounting, and "-1s" would be nonsense.
          -->
          <span class="telemetry text-outline">Declines in {{ Math.max(0, remaining) }}s</span>
          <div class="flex items-center gap-space-sm">
            <button
              type="button"
              class="flex items-center gap-space-xs rounded-md px-space-md py-space-sm font-label-md text-label-md text-on-surface-variant transition-colors hover:bg-error-container/20 hover:text-on-error-container"
              @click="transfers.respond(offer.id, false)"
            >
              <X :size="16" />
              <span>Decline</span>
            </button>
            <button
              type="button"
              class="flex items-center gap-space-xs rounded-md bg-primary-container px-space-lg py-space-sm font-label-md text-label-md font-semibold text-on-primary-container transition-colors hover:bg-primary hover:text-on-primary"
              @click="transfers.respond(offer.id, true)"
            >
              <Check :size="16" />
              <span>Accept</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>
