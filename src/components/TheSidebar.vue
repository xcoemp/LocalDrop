<!--
  TheSidebar.vue — primary navigation, rendered as a desktop rail or a mobile
  bottom bar (UI-5), plus at-a-glance network and session telemetry.

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { Radar, RefreshCcw, Settings, Upload, Waypoints } from "lucide-vue-next";

import ToggleSwitch from "@/components/ToggleSwitch.vue";
import { formatBytes } from "@/composables/useFormat";
import { usePeerStore } from "@/stores/usePeerStore";
import { useSettingsStore } from "@/stores/useSettingsStore";
import { useTransferStore } from "@/stores/useTransferStore";

export type ViewId = "radar" | "send" | "transfers" | "settings";

const model = defineModel<ViewId>({ required: true });

const peers = usePeerStore();
const transfers = useTransferStore();
const settings = useSettingsStore();

/**
 * The four screens, as data rather than four hand-written buttons.
 *
 * Driving both layouts from one array is the point: the rail and the bottom bar
 * below iterate the same list, so a screen cannot be added to one and forgotten
 * in the other. `as const` on each id keeps the union narrow enough to assign
 * straight to `model`.
 */
const nav = [
  { id: "radar" as const, label: "Discovery Radar", icon: Radar },
  { id: "send" as const, label: "Send Payload", icon: Upload },
  { id: "transfers" as const, label: "Transfer Streams", icon: Waypoints },
  { id: "settings" as const, label: "Settings", icon: Settings },
];

const activeCount = computed(() => transfers.running.length);

/**
 * Bytes moved this session, for the rail's footer.
 *
 * Filter then fold: only `completed` entries count, because a transfer that
 * failed at 90% did not move those bytes to a usable destination — the
 * receiver's `.part` file was deleted (§2.2). Counting them would overstate
 * what the session actually achieved.
 */
const sessionBytes = computed(() =>
  transfers.history
    .filter((h) => h.outcome === "completed")
    .reduce((sum, h) => sum + h.totalBytes, 0),
);

/**
 * SEC-6 — auto-accept, also exposed here because it is consequential enough to
 * be reachable without opening Settings.
 *
 * Writable computed: the getter defaults to `false` before hydration, matching
 * the schema default so the switch never renders on for a user who has it off.
 */
const autoAccept = computed({
  get: () => settings.settings?.autoAccept ?? false,
  set: (value: boolean) => void settings.update({ autoAccept: value }),
});
</script>

<template>
  <!-- UI-5 — desktop rail. Below lg the same nav renders as a bottom bar. -->
  <aside
    class="fixed bottom-0 left-0 top-16 z-40 hidden w-72 flex-col justify-between border-r border-outline-variant/40 bg-surface-container-lowest/80 p-space-md backdrop-blur-md lg:flex"
  >
    <div class="flex flex-col gap-space-md">
      <div class="px-space-xs">
        <div class="mb-space-xs font-label-sm text-label-sm uppercase tracking-wider text-outline">
          Network scope
        </div>
        <div
          class="flex items-center justify-between gap-space-sm rounded-md border border-outline-variant/30 bg-surface-container-low p-space-sm"
        >
          <div class="flex min-w-0 items-center gap-space-xs">
            <!-- Online state as colour, with the interface name beside it doing
                 the actual communicating (UI-7). -->
            <span
              class="h-2 w-2 shrink-0 rounded-full"
              :class="peers.online ? 'bg-primary' : 'bg-error'"
            />
            <span class="truncate font-label-md text-label-md font-medium text-on-surface">
              {{ peers.network?.interface ?? "No interface" }}
            </span>
          </div>
          <span
            class="telemetry shrink-0 rounded-sm bg-surface-container-high px-space-xs py-0.5 text-primary"
          >
            {{ peers.network?.netmask ?? "—" }}
          </span>
        </div>
      </div>

      <nav class="flex flex-col gap-1" aria-label="Primary">
        <!--
          Repetition over `nav`. The active item is marked three ways: a
          background, a heavier weight, and `aria-current="page"` — the last is
          what a screen reader announces, since the visual cues are invisible to
          it (UI-7). `undefined` rather than `false` so the attribute is omitted
          entirely on inactive items, as the ARIA spec expects.
        -->
        <button
          v-for="item in nav"
          :key="item.id"
          type="button"
          :aria-current="model === item.id ? 'page' : undefined"
          class="group flex items-center justify-between rounded-md px-space-md py-space-sm transition-all"
          :class="
            model === item.id
              ? 'bg-primary-container font-semibold text-on-primary-container aura-ready'
              : 'text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface'
          "
          @click="model = item.id"
        >
          <span class="flex items-center gap-space-md">
            <component
              :is="item.icon"
              :size="20"
              class="transition-transform group-hover:scale-110"
            />
            <span class="font-label-md text-label-md">{{ item.label }}</span>
          </span>
          <!--
            Per-item count badges. Each tests the item id as well as its count,
            because this runs inside the shared loop — only Transfers and Radar
            have a number worth showing, and the two are styled differently
            (Transfers is emphasised, Radar is informational). Chained with
            `v-else-if` since an item is never both.

            The truthiness test on the count doubles as a zero check: a badge
            reading "0" would be noise.
          -->
          <span
            v-if="item.id === 'transfers' && activeCount"
            class="telemetry rounded-full border border-primary/30 bg-primary-container/20 px-space-xs py-0.5 text-primary"
          >
            {{ activeCount }}
          </span>
          <span v-else-if="item.id === 'radar' && peers.list.length" class="telemetry text-outline">
            {{ peers.list.length }}
          </span>
        </button>
      </nav>

      <!--
        FR-1.6 — Rescan. Disabled during its own 1.2 s window so an impatient
        double-press cannot clear the table twice and discard peers that had
        just reappeared. Icon spins and label changes, so the disabled state has
        a visible reason.
      -->
      <button
        type="button"
        class="mx-space-xs flex items-center justify-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface transition-colors hover:bg-surface-bright disabled:opacity-50"
        :disabled="peers.scanning"
        @click="peers.rescan()"
      >
        <RefreshCcw :size="16" :class="peers.scanning ? 'animate-spin' : ''" />
        <span>{{ peers.scanning ? "Scanning…" : "Rescan subnet" }}</span>
      </button>
    </div>

    <div class="flex flex-col gap-space-md border-t border-outline-variant/40 pt-space-md">
      <div
        class="flex items-center justify-between gap-space-sm rounded-md border border-outline-variant/30 bg-surface-container-low p-space-sm"
      >
        <div class="flex min-w-0 flex-col">
          <span class="font-label-md text-label-md font-medium text-on-surface">
            Quick drop receiver
          </span>
          <span class="font-label-sm text-label-sm text-outline">Auto-accept LAN shares</span>
        </div>
        <ToggleSwitch v-model="autoAccept" label="Auto-accept incoming transfers" />
      </div>

      <div class="flex flex-col gap-space-xs px-space-xs">
        <div class="flex items-center justify-between text-on-surface-variant">
          <span class="font-label-sm text-label-sm">Active TCP streams</span>
          <span class="telemetry font-bold text-primary">{{ activeCount }}</span>
        </div>
        <div class="flex items-center justify-between text-on-surface-variant">
          <span class="font-label-sm text-label-sm">Transferred this session</span>
          <span class="telemetry font-semibold text-on-surface">
            {{ formatBytes(sessionBytes) }}
          </span>
        </div>
      </div>

      <!-- Footer status line: the third and last place `online` is surfaced
           (header, scope panel, here), each at a different altitude. -->
      <div class="telemetry flex items-center justify-between px-space-xs text-outline">
        <span class="flex items-center gap-1">
          <span
            class="h-1.5 w-1.5 rounded-full"
            :class="peers.online ? 'bg-primary' : 'bg-error'"
          />
          {{ peers.online ? "listener ready" : "offline" }}
        </span>
        <!-- The bound transfer port, which §6.2's fallback may have moved. -->
        <span>:{{ peers.network?.transferPort ?? "—" }}</span>
      </div>
    </div>
  </aside>

  <!-- Mobile bottom navigation (UI-5).
       The bottom inset is essential, not cosmetic: without it this sits behind
       the Android navigation bar and its taps never reach the app, which makes
       every tab except the first unreachable. -->
  <nav
    class="fixed inset-x-0 bottom-0 z-40 flex border-t border-outline-variant/40 bg-surface-container-lowest/95 backdrop-blur-xl lg:hidden"
    aria-label="Primary"
    :style="{
      paddingBottom: 'var(--safe-bottom)',
      paddingLeft: 'var(--safe-left)',
      paddingRight: 'var(--safe-right)',
    }"
  >
    <!--
      The same `nav` array, iterated a second time for the mobile layout. Two
      markup blocks rather than one responsive block because the structures
      genuinely differ — a horizontal icon-over-label tab bar versus a vertical
      labelled rail with badges — and forcing both through one template would
      need more conditionals than it saves.
    -->
    <button
      v-for="item in nav"
      :key="item.id"
      type="button"
      :aria-current="model === item.id ? 'page' : undefined"
      class="relative flex flex-1 flex-col items-center justify-center gap-1 py-space-sm transition-colors"
      :style="{ minHeight: 'var(--bottom-nav-h)' }"
      :class="model === item.id ? 'text-primary' : 'text-on-surface-variant'"
      @click="model = item.id"
    >
      <component :is="item.icon" :size="20" />
      <!-- First word only: "Discovery Radar" will not fit in a quarter of a
           phone's width, and the first word is the distinguishing one. -->
      <span class="font-label-sm text-label-sm">{{ item.label.split(" ")[0] }}</span>
      <!-- A dot rather than a count here: at this size the numeral would be
           illegible, and its presence is the useful signal. -->
      <span
        v-if="item.id === 'transfers' && activeCount"
        class="absolute right-1/4 top-1 h-2 w-2 rounded-full bg-primary"
      />
    </button>
  </nav>
</template>
