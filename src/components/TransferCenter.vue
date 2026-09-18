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
import { Activity, Folder, Inbox, Trash2 } from "lucide-vue-next";

import EmptyState from "@/components/EmptyState.vue";
import HistoryRow from "@/components/HistoryRow.vue";
import TransferRow from "@/components/TransferRow.vue";
import { formatBitrate, formatRate } from "@/composables/useFormat";
import { useSettingsStore } from "@/stores/useSettingsStore";
import { useTransferStore } from "@/stores/useTransferStore";

const transfers = useTransferStore();
const settings = useSettingsStore();

/** FR-5.3 — aggregate throughput across every live stream. */
const aggregate = computed(() => transfers.aggregateRate);
// `running`, not `activeList`: rows linger for 4 s after finishing, and
// counting those as channels would overstate what is actually in flight.
const channels = computed(() => transfers.running.length);

/**
 * FR-6.2's download directory, surfaced next to the completed list.
 *
 * This is the screen a user lands on when a transfer finishes and their next
 * question is "so where is it?" — on Android especially, where there is no
 * reveal-in-folder action to answer it for them. Reads the live setting rather
 * than a copy, so changing the folder updates the label immediately.
 *
 * Empty string until the store hydrates, which the template treats as
 * "nothing to show" rather than rendering a bare label with no path.
 */
const downloadDir = computed(() => settings.settings?.downloadDirectory ?? "");
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

        <!--
          `flex-wrap` matters at narrow widths. Without it the three facts are
          forced onto one line, and since they cannot move they each break
          internally instead — "0 active\nchannels", "TCP chunks\n128 KB".
          Wrapping lets the row break at the bullet separators, which is where
          a reader expects it.
        -->
        <div class="telemetry flex flex-wrap items-center gap-space-xs text-on-surface-variant">
          <!-- Inline pluralisation; the two facts beside it are constants from
               §6.3, included because the mockup shows the transport in use. -->
          <span class="whitespace-nowrap font-semibold text-primary">
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

      <!--
        Where received files land. Shown above the list rather than inside the
        header row, which already carries Clear history and would crowd.

        Rendered whether or not there is any history: with an empty list it
        still answers "where will things arrive?", and with a full one it
        answers "where did they go?".

        Deliberately says *received*: outgoing entries in the list below were
        read from wherever the sender picked them, not from here.

        LAYOUT: the path wraps, it does not truncate. An earlier version was a
        flex row with `shrink-0` on the label and a character-count truncation
        on the path, which overflowed horizontally in a narrow window — and
        because the overflow widened the whole page, it dragged the section
        headings off the left edge rather than just clipping the path.
        So: `min-w-0` on both the flex child and the paragraph (a flex item
        defaults to `min-width: auto` and refuses to shrink below its content),
        the label and path as normal inline text in one <p> so they reflow
        together, and `wrap-anywhere` so a path with no spaces still breaks
        rather than pushing the layout wide. `wrap-anywhere` in preference to
        `break-all`, which breaks mid-word even where a break is unnecessary and
        would split "Downloads" across two lines for no reason.
        Full value stays in the tooltip, and
        `data-selectable` re-enables selection (the app suppresses it globally)
        so the path can be copied into a file manager.

        `block sm:inline` on the path puts it on its own line below sm. Inline
        on a phone, the sentence ran the label and the path together and the
        path then broke mid-way through, so neither read cleanly; a line of its
        own gives it the full width and usually removes the break entirely.
        Above sm there is room for one line, and inline is tidier.
      -->
      <div v-if="downloadDir" class="flex min-w-0 items-start gap-space-xs px-space-xs">
        <Folder :size="14" class="mt-0.5 shrink-0 text-outline" />
        <p class="min-w-0 font-body-sm text-body-sm text-on-surface-variant">
          Received files are saved to
          <span
            data-selectable
            class="telemetry block wrap-anywhere text-on-surface sm:inline"
            :title="downloadDir"
            >{{ downloadDir }}</span
          >
        </p>
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
