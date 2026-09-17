<!--
  TransferCenter.vue — the Transfers screen: aggregate throughput, active
  pipelines, and completed activity (FR-5.1 through FR-5.6).

  Serves as the "Transfer Streams" view in the sidebar; it lives in components/
  rather than views/ because the shell mounts it directly.

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { Activity, Inbox, Trash2 } from "lucide-vue-next";

import EmptyState from "@/components/EmptyState.vue";
import HistoryRow from "@/components/HistoryRow.vue";
import TransferRow from "@/components/TransferRow.vue";
import { formatBitrate, formatRate } from "@/composables/useFormat";
import { useTransferStore } from "@/stores/useTransferStore";

const transfers = useTransferStore();

/** FR-5.3 — aggregate throughput across every live stream. */
const aggregate = computed(() => transfers.aggregateRate);
// `running`, not `activeList`: rows linger for 4 s after finishing, and
// counting those as channels would overstate what is actually in flight.
const channels = computed(() => transfers.running.length);
</script>

<template>
  <div class="flex flex-col gap-space-lg">
    <!-- Usage statistics header: aggregate throughput across all active
         streams (FR-5.3), from the transfer_center_pipeline_monitor mockup. -->
    <section
      class="relative overflow-hidden rounded-lg bg-surface-container-lowest/70 p-space-lg backdrop-blur-xl"
    >
      <div
        class="pointer-events-none absolute -right-16 -top-16 h-64 w-64 rounded-full bg-primary/10 blur-3xl"
      />
      <div class="relative z-10 flex flex-col gap-space-xs">
        <div
          class="flex items-center gap-space-xs font-label-sm text-label-sm uppercase tracking-widest text-on-surface-variant"
        >
          <!-- Pulses only when something is actually moving; a static dot
               otherwise, so the header is not perpetually animating. -->
          <span
            class="h-2 w-2 rounded-full"
            :class="channels ? 'animate-pulse bg-primary' : 'bg-outline'"
          />
          Usage Statistics
        </div>

        <div class="flex items-baseline gap-space-xs">
          <!--
            The formatted rate is split so the numeral and its unit can be
            styled and sized independently, as the mockup does. `tabular-nums`
            is UI-2: fixed-width digits, so the figure does not jitter sideways
            as it changes ten times a second.
          -->
          <span
            class="font-headline-xl text-headline-xl font-bold tracking-tight text-on-surface tabular-nums"
          >
            {{ formatRate(aggregate).split(" ")[0] }}
          </span>
          <span class="font-headline-md text-headline-md font-semibold text-primary">
            {{ formatRate(aggregate).split(" ")[1] }}
          </span>
          <span
            class="telemetry ml-space-sm rounded-sm bg-surface-container-high px-space-xs py-0.5 text-on-surface-variant"
          >
            {{ formatBitrate(aggregate) }}
          </span>
        </div>

        <div class="telemetry flex items-center gap-space-xs text-on-surface-variant">
          <!-- Inline pluralisation; the two facts beside it are constants from
               §6.3, included because the mockup shows the transport in use. -->
          <span class="font-semibold text-primary">
            {{ channels }} active channel{{ channels === 1 ? "" : "s" }}
          </span>
          <span>•</span>
          <span>TCP chunks 128 KB</span>
          <span>•</span>
          <span>Direct peer socket</span>
        </div>
      </div>
    </section>

    <!-- Active pipelines -->
    <section class="flex flex-col gap-space-sm">
      <h2
        class="px-space-xs font-label-sm text-label-sm font-semibold uppercase tracking-widest text-outline"
      >
        Active pipelines
      </h2>

      <!--
        Selection between the empty state and the rows (UI-4). Note the `v-else`
        sits on the `v-for` element: Vue evaluates `v-for` first, so this reads
        as "if there are none, show the placeholder; otherwise render each".
      -->
      <EmptyState
        v-if="!transfers.activeList.length"
        :icon="Activity"
        title="Nothing in flight"
        body="Drop files on a peer, or pick one from the radar, and progress shows up here."
      />

      <!--
        Iterates `activeList` (sorted, includes lingering finished rows) rather
        than `running`, so a completed transfer stays visible for its 4 s and
        the user sees the outcome instead of the row vanishing at 100%.
      -->
      <TransferRow
        v-for="transfer in transfers.activeList"
        v-else
        :key="transfer.id"
        :transfer="transfer"
      />
    </section>

    <!-- Completed activity -->
    <section class="flex flex-col gap-space-sm">
      <div class="flex items-center justify-between px-space-xs">
        <h2
          class="font-label-sm text-label-sm font-semibold uppercase tracking-widest text-outline"
        >
          Completed activity
        </h2>
        <!-- FR-5.4's Clear History, hidden when there is nothing to clear
             rather than disabled — a dead button invites a pointless click. -->
        <button
          v-if="transfers.history.length"
          type="button"
          class="flex items-center gap-space-xs rounded-md px-space-sm py-1 font-label-md text-label-md text-on-surface-variant transition-colors hover:bg-surface-container-high hover:text-on-surface"
          @click="transfers.clearHistory()"
        >
          <Trash2 :size="14" />
          <span>Clear history</span>
        </button>
      </div>

      <EmptyState
        v-if="!transfers.history.length"
        :icon="Inbox"
        title="No transfers yet"
        body="Completed and failed transfers are listed here, with a link to each received file."
      />

      <!--
        Repetition over the persisted history, newest first (the store prepends)
        and capped at 200 entries by Rust, so this list is bounded regardless of
        how long the app has been in use.
      -->
      <div v-else class="flex flex-col gap-1">
        <HistoryRow v-for="entry in transfers.history" :key="entry.id" :entry="entry" />
      </div>
    </section>
  </div>
</template>
