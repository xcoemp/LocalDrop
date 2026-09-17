<!--
  PeerCard.vue — one discovered device on the radar (§10.2): identity, live
  status, per-peer telemetry, and the two send actions.

  Also the drop target for UI-1: the `data-peer-id` attribute is what
  `useDragDrop`'s hit-testing looks for when walking up from the hovered node.

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { ClipboardPaste, Hourglass, Upload } from "lucide-vue-next";

import OsIcon from "@/components/OsIcon.vue";
import { formatPercent, formatRate, truncateMiddle } from "@/composables/useFormat";
import type { Peer, TransferSnapshot } from "@/types/protocol";

const props = defineProps<{
  peer: Peer;
  selected: boolean;
  /** UI-1 — a file drag is hovering this card. */
  dropTarget: boolean;
  transfer: TransferSnapshot | null;
}>();

const emit = defineEmits<{
  select: [deviceId: string];
  sendFiles: [deviceId: string];
  sendText: [deviceId: string];
}>();

/**
 * Selection over the peer's OS, for the badge text and the accessible name.
 *
 * Duplicated from OsIcon's own `label` rather than read off the child, because
 * the parent needs the string in `aria-label` before the child renders. The
 * default arm reads "Unknown OS" rather than OsIcon's bare "Unknown", since
 * here it appears as a standalone badge where "Unknown" alone would be unclear.
 */
const osLabel = computed(() => {
  switch (props.peer.os) {
    case "android":
      return "Android";
    case "windows":
      return "Windows";
    case "macos":
      return "macOS";
    case "linux":
      return "Linux";
    default:
      return "Unknown OS";
  }
});

/**
 * §6.2 — a `busy` peer is still visible but not a drop destination, so a drag
 * never lands on a device that cannot accept it.
 *
 * The second clause is what makes the distinction useful: a peer we are
 * ourselves transferring to also reports `busy`, but that card should show live
 * progress, not a greyed-out "Busy". So `busy` here means "occupied with
 * someone else" — the state the user can do nothing about.
 */
const busy = computed(() => props.peer.state === "busy" && !props.transfer);

const percent = computed(() =>
  // Guarded because `transfer` is null for an idle peer; `formatPercent` itself
  // handles the zero-total case for empty-file batches.
  props.transfer ? formatPercent(props.transfer.bytesDone, props.transfer.totalBytes) : 0,
);
</script>

<template>
  <div
    :data-peer-id="peer.deviceId"
    class="group relative flex flex-col justify-between overflow-hidden rounded-lg p-space-md transition-all"
    :class="[
      /*
        Three independent visual states, applied as an array rather than a
        chain: a card can be a drop target *and* selected *and* transferring at
        once, so these must compose instead of overriding one another. The first
        entry is a genuine either/or (hover styling replaces the resting
        background); the other two contribute only when their condition holds.
      */
      dropTarget
        ? 'bg-surface-container ring-2 ring-primary aura-ready scale-[1.01]'
        : 'bg-surface-container-low hover:bg-surface-container',
      selected ? 'ring-1 ring-primary/60' : '',
      transfer ? 'aura-active' : '',
    ]"
    role="button"
    tabindex="0"
    :aria-label="`${peer.alias}, ${osLabel}, ${peer.ip}`"
    @click="emit('select', peer.deviceId)"
    @keydown.enter.prevent="emit('select', peer.deviceId)"
    @keydown.space.prevent="emit('select', peer.deviceId)"
  >
    <!-- Identity -->
    <div class="mb-space-md flex items-start justify-between gap-space-sm">
      <div class="flex min-w-0 items-center gap-space-sm">
        <div
          class="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-surface-container-high text-primary transition-colors group-hover:bg-primary-container group-hover:text-on-primary-container"
        >
          <OsIcon :os="peer.os" :size="22" />
        </div>
        <div class="flex min-w-0 flex-col">
          <span
            class="truncate font-headline-md text-headline-md font-semibold text-on-surface"
            :title="peer.alias"
          >
            {{ peer.alias }}
          </span>
          <div class="flex items-center gap-space-xs">
            <span
              class="rounded-sm bg-surface-container-highest px-1 font-label-sm text-label-sm uppercase text-on-surface-variant"
            >
              {{ osLabel }}
            </span>
            <!-- §6.6 — the IP disambiguates two devices sharing an alias. -->
            <span class="telemetry text-outline">{{ peer.ip }}</span>
          </div>
        </div>
      </div>

      <!-- UI-7 — the dot is always paired with a text label. -->
      <div
        class="flex shrink-0 items-center gap-1 rounded-full bg-surface-container-high px-space-xs py-0.5"
      >
        <span class="relative flex h-2 w-2">
          <!--
            The pinging halo is suppressed for a busy peer: an animated pulse
            reads as "available", which is the opposite of what Busy means.
            Idle and active both keep it, differing only in colour.
          -->
          <span
            v-if="!busy"
            class="absolute inline-flex h-full w-full animate-ping rounded-full opacity-75"
            :class="transfer ? 'bg-tertiary' : 'bg-primary'"
          />
          <!--
            Nested ternary over the three states — busy, transferring, ready —
            repeated below for the label. Written twice rather than hoisted to a
            computed pair because each consumer needs a different vocabulary
            (Tailwind class vs. human word) and the order of tests is the
            important part: busy is checked first because it is the state that
            disables the card's actions.
          -->
          <span
            class="relative inline-flex h-2 w-2 rounded-full"
            :class="busy ? 'bg-outline' : transfer ? 'bg-tertiary' : 'bg-primary'"
          />
        </span>
        <span
          class="telemetry font-bold"
          :class="busy ? 'text-outline' : transfer ? 'text-tertiary' : 'text-primary'"
        >
          {{ busy ? "Busy" : transfer ? "Active" : "Ready" }}
        </span>
      </div>
    </div>

    <!--
      Status strip. Selection between live telemetry and the idle line, keeping
      the card's height identical either way — without a same-sized substitute,
      every card in the grid would reflow the moment one began transferring.
    -->
    <div
      v-if="transfer"
      class="mb-space-md flex flex-col space-y-1.5 rounded-md bg-surface-container-lowest p-space-sm"
    >
      <div class="flex items-center justify-between">
        <!--
          `|| label` covers the gap between a transfer being accepted and its
          first file opening, when `currentFile` is still empty.
        -->
        <span class="truncate font-label-sm text-label-sm text-tertiary">
          {{ truncateMiddle(transfer.currentFile || transfer.label, 26) }}
        </span>
        <span class="telemetry shrink-0 font-bold text-tertiary">
          {{ Math.round(percent) }}% ({{ formatRate(transfer.rateBps) }})
        </span>
      </div>
      <div class="h-1.5 w-full overflow-hidden rounded-full bg-surface-container-high">
        <div
          class="progress-fill h-full rounded-full bg-tertiary"
          :style="{ width: `${percent}%` }"
        />
      </div>
    </div>

    <div
      v-else
      class="mb-space-md flex items-center justify-between rounded-md bg-surface-container-lowest px-space-sm py-space-xs"
    >
      <div class="flex items-center gap-space-xs text-on-surface-variant">
        <Hourglass :size="16" class="text-outline" />
        <!--
          Explains *why* the peer is unavailable rather than just labelling it.
          "Receiving from another device" tells the user to wait; a bare "Busy"
          would leave them wondering whether something is broken.
        -->
        <span class="font-label-sm text-label-sm">
          {{ busy ? "Receiving from another device" : "Idle standby listener" }}
        </span>
      </div>
      <span class="telemetry text-outline">{{ busy ? "Busy" : "Listening" }}</span>
    </div>

    <!--
      Actions. Both disabled while the peer is busy — §6.2 makes a busy peer
      non-droppable, and the buttons must agree with the drag behaviour or the
      two paths would contradict each other.

      `@click.stop` on each: the whole card is a click target for selection, and
      without stopping propagation pressing Send would also re-fire select.
    -->
    <div class="flex items-center gap-space-xs pt-space-xs">
      <button
        type="button"
        class="flex flex-1 items-center justify-center gap-1 rounded-md bg-primary-container py-1.5 font-label-md text-label-md font-semibold text-on-primary-container transition-colors hover:bg-primary hover:text-on-primary disabled:cursor-not-allowed disabled:opacity-50"
        :disabled="busy"
        @click.stop="emit('sendFiles', peer.deviceId)"
      >
        <Upload :size="16" />
        <span>Send Files</span>
      </button>
      <button
        type="button"
        class="flex items-center justify-center rounded-md bg-surface-container-high p-1.5 text-on-surface transition-colors hover:bg-surface-bright disabled:cursor-not-allowed disabled:opacity-50"
        title="Send clipboard snippet"
        aria-label="Send clipboard snippet"
        :disabled="busy"
        @click.stop="emit('sendText', peer.deviceId)"
      >
        <ClipboardPaste :size="18" />
      </button>
    </div>
  </div>
</template>
