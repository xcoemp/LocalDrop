<!--
  TransferRow.vue — one active transfer: direction, filename, `n of m`,
  percentage, rate, ETA, and cancel (FR-5.1).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { ArrowDownToLine, ArrowUpFromLine, Ban, CircleCheck, CircleX, X } from "lucide-vue-next";

import { formatBytes, formatEta, formatPercent, formatRate } from "@/composables/useFormat";
import { useTransferStore } from "@/stores/useTransferStore";
import type { TransferSnapshot } from "@/types/protocol";

const props = defineProps<{ transfer: TransferSnapshot }>();
const transfers = useTransferStore();

const percent = computed(() => formatPercent(props.transfer.bytesDone, props.transfer.totalBytes));

/**
 * Whether the transfer can still be cancelled (FR-2.7).
 *
 * Membership test over the three non-terminal phases rather than negating the
 * four terminal ones — an added phase then defaults to *not* cancellable, which
 * is the safe direction: offering to cancel something already finished would
 * send a pointless command, while a missing button is merely a missing button.
 */
const live = computed(() => ["queued", "offered", "transferring"].includes(props.transfer.phase));

/**
 * §6.4 — both ends label the phase identically.
 *
 * The `transferring` arm is the only one that interpolates: `n of m` is the
 * per-item progress FR-5.1 requires, and it is meaningless in every other
 * phase. `failed` is the `default` arm rather than an explicit case, so an
 * unrecognised phase from a newer peer reads as a failure instead of rendering
 * blank — an unlabelled row would leave the user unable to tell what happened.
 */
const phaseLabel = computed(() => {
  switch (props.transfer.phase) {
    case "queued":
      return "Queued";
    case "offered":
      // Waiting on the remote user's accept dialog, not on the network.
      return "Waiting for the other device…";
    case "transferring":
      return `${props.transfer.index} of ${props.transfer.count}`;
    case "completed":
      return "Completed";
    case "cancelled":
      return "Cancelled";
    case "rejected":
      // "Declined" reads as a person's decision, which is what it was.
      return "Declined";
    default:
      return "Failed";
  }
});

/**
 * Text accent per phase.
 *
 * `failed` and `rejected` deliberately fall through to the same arm: both are
 * outcomes the user did not want. `cancelled` is muted rather than red, since
 * it was their own choice and is not an error. The `default` covers the three
 * in-flight phases, which share the active tertiary colour.
 */
const accent = computed(() => {
  switch (props.transfer.phase) {
    case "completed":
      return "text-primary";
    case "failed":
    case "rejected":
      return "text-error";
    case "cancelled":
      return "text-outline";
    default:
      return "text-tertiary";
  }
});

/** The same phase selection again, for the progress bar fill. */
const barClass = computed(() => {
  switch (props.transfer.phase) {
    case "completed":
      return "bg-primary";
    case "failed":
    case "rejected":
      return "bg-error";
    case "cancelled":
      return "bg-outline";
    default:
      // DESIGN.md — active fill runs sky blue into emerald.
      return "bg-gradient-to-r from-tertiary-container to-primary-container";
  }
});
</script>

<template>
  <div class="flex flex-col gap-space-sm rounded-lg bg-surface-container-low p-space-md">
    <div class="flex items-start justify-between gap-space-md">
      <!-- `items-start` so the icon tile stays beside the first line of a
           filename that wraps, rather than drifting to the block's centre. -->
      <div class="flex min-w-0 items-start gap-space-sm">
        <div
          class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-surface-container-high"
          :class="accent"
        >
          <!--
            Nested selection, outcome before direction: once a transfer has
            finished, *what happened* matters more than which way it went, so
            the terminal phases each claim their own icon and only an in-flight
            row falls through to the directional arrow.

            Kept in the template rather than a computed because it mirrors the
            `accent` and `barClass` selections above and reads best beside them.
          -->
          <component
            :is="
              transfer.phase === 'completed'
                ? CircleCheck
                : transfer.phase === 'failed' || transfer.phase === 'rejected'
                  ? CircleX
                  : transfer.phase === 'cancelled'
                    ? Ban
                    : transfer.direction === 'incoming'
                      ? ArrowDownToLine
                      : ArrowUpFromLine
            "
            :size="18"
          />
        </div>
        <div class="flex min-w-0 flex-col">
          <!--
            The current file while transferring, falling back to the batch label
            before the first file opens or after the last one closes.

            Wraps instead of truncating. This name changes as the batch advances
            and is the only indication of *which* file is moving, so clipping
            its tail on a narrow screen removes the one fact the line exists to
            convey. `wrap-anywhere` because filenames frequently contain no
            break opportunity at all.
          -->
          <span class="wrap-anywhere font-label-md text-label-md text-on-surface">
            {{ transfer.currentFile || transfer.label }}
          </span>
          <span class="telemetry wrap-anywhere text-on-surface-variant">
            {{ transfer.direction === "incoming" ? "from" : "to" }}
            {{ transfer.peerAlias }} · {{ phaseLabel }}
          </span>
        </div>
      </div>

      <div class="flex shrink-0 items-center gap-space-md">
        <!-- Hidden on narrow screens: the byte counts would wrap and push the
             cancel button off the row (UI-5). -->
        <div class="hidden flex-col items-end sm:flex">
          <span class="telemetry font-bold" :class="accent"> {{ Math.round(percent) }}% </span>
          <span class="telemetry text-on-surface-variant">
            {{ formatBytes(transfer.bytesDone) }} / {{ formatBytes(transfer.totalBytes) }}
          </span>
        </div>

        <!-- FR-2.7 — cancellable from either end, at any point. -->
        <button
          v-if="live"
          type="button"
          class="rounded-md p-1.5 text-on-surface-variant transition-colors hover:bg-error-container/20 hover:text-on-error-container"
          :aria-label="`Cancel transfer of ${transfer.label}`"
          title="Cancel"
          @click="transfers.cancel(transfer.id)"
        >
          <X :size="16" />
        </button>
      </div>
    </div>

    <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-container-high">
      <!--
        The pulse animation is applied only while actually transferring: a
        pulsing bar on a queued or finished row would imply movement that is not
        happening. Width is bound to the clamped percentage, which UI-3 requires
        never run backwards.
      -->
      <div
        class="progress-fill h-full rounded-full"
        :class="[barClass, transfer.phase === 'transferring' ? 'animate-pulse' : '']"
        :style="{ width: `${percent}%` }"
      />
    </div>

    <div class="flex items-center justify-between">
      <!--
        The footer swaps between live telemetry and the phase label: a rate is
        only meaningful while bytes are moving, and "0 MB/s" under a completed
        transfer would read as a stall.
      -->
      <span class="telemetry" :class="accent">
        {{ transfer.phase === "transferring" ? formatRate(transfer.rateBps) : phaseLabel }}
      </span>
      <!-- ETA is omitted entirely rather than shown as "—" outside transfer,
           since the label beside it already explains the state. -->
      <span v-if="transfer.phase === 'transferring'" class="telemetry text-on-surface-variant">
        ETA {{ formatEta(transfer.etaSeconds) }}
      </span>
    </div>
  </div>
</template>
