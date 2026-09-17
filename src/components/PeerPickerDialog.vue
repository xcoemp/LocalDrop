<!--
  PeerPickerDialog.vue — destination chooser for an ambiguous drop (UI-1).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import OsIcon from "@/components/OsIcon.vue";
import { usePeerStore } from "@/stores/usePeerStore";

/**
 * UI-1 — files dropped on empty canvas need a destination. Asking beats
 * guessing: sending to the wrong device is not undoable.
 */
defineProps<{ paths: string[] }>();
const emit = defineEmits<{ pick: [deviceId: string]; cancel: [] }>();

const peers = usePeerStore();
</script>

<template>
  <!--
    Visibility is derived from the payload itself rather than a separate `open`
    flag: the dialog exists exactly when there are paths awaiting a destination,
    so the two can never disagree. `useDragDrop` opens it by parking paths and
    closes it by clearing them.

    `@click.self` — only a click on the backdrop itself cancels. Without
    `.self`, any click that bubbled up from the dialog body would dismiss it,
    including the peer buttons.
  -->
  <div
    v-if="paths.length"
    class="fixed inset-0 z-60 flex items-center justify-center bg-[rgba(9,13,22,0.75)] p-space-md backdrop-blur-[8px]"
    role="dialog"
    aria-modal="true"
    aria-label="Choose a destination device"
    @click.self="emit('cancel')"
    @keydown.esc="emit('cancel')"
  >
    <div
      class="flex w-full max-w-md flex-col gap-space-md rounded-xl border border-white/15 bg-surface-container p-space-lg"
    >
      <header class="flex flex-col">
        <h2 class="font-headline-lg text-headline-lg text-on-surface">Send to which device?</h2>
        <span class="telemetry text-on-surface-variant">
          <!-- Inline pluralisation; the count confirms what was actually
               dropped, since the drag itself is already over. -->
          {{ paths.length }} item{{ paths.length === 1 ? "" : "s" }} ready
        </span>
      </header>

      <div class="flex max-h-72 flex-col gap-space-xs overflow-y-auto">
        <!--
          Repetition over every known peer — `list`, not `visible`, because the
          radar's text filter is unrelated to this decision and hiding a valid
          destination behind a stale filter would be surprising.

          A busy peer is shown but disabled rather than omitted: seeing it
          greyed out with a reason explains its absence, where dropping it from
          the list would just look like the device had gone offline (§6.2).
        -->
        <button
          v-for="peer in peers.list"
          :key="peer.deviceId"
          type="button"
          class="flex items-center gap-space-sm rounded-md bg-surface-container-lowest p-space-sm text-left transition-colors hover:bg-surface-container-high disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="peer.state === 'busy'"
          @click="emit('pick', peer.deviceId)"
        >
          <div
            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-surface-container-high text-primary"
          >
            <OsIcon :os="peer.os" :size="18" />
          </div>
          <div class="flex min-w-0 flex-col">
            <span class="truncate font-label-md text-label-md text-on-surface">
              {{ peer.alias }}
            </span>
            <span class="telemetry text-outline">
              <!-- UI-7 — the disabled state is also stated in words, not left
                   to opacity alone. -->
              {{ peer.ip }}{{ peer.state === "busy" ? " · busy" : "" }}
            </span>
          </div>
        </button>
      </div>

      <div class="flex justify-end">
        <button
          type="button"
          class="rounded-md px-space-md py-space-sm font-label-md text-label-md text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface"
          @click="emit('cancel')"
        >
          Cancel
        </button>
      </div>
    </div>
  </div>
</template>
