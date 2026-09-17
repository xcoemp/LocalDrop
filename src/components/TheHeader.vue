<!--
  TheHeader.vue — the fixed top bar: wordmark, this device's identity, network
  scope, and the two trust indicators (§8.1, SEC-6).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { ShieldAlert, Wifi, WifiOff } from "lucide-vue-next";

import OsIcon from "@/components/OsIcon.vue";
import { usePeerStore } from "@/stores/usePeerStore";
import { useSettingsStore } from "@/stores/useSettingsStore";

const peers = usePeerStore();
const settings = useSettingsStore();

// Both are nullable until the stores hydrate, which is why every read in the
// template below is guarded or defaulted. `device.alias` is derived in the
// store from the live setting, so renaming in Settings updates this header
// without a refetch.
const device = computed(() => settings.device);
const network = computed(() => peers.network);
</script>

<template>
  <!-- Height and padding absorb the status-bar inset so the wordmark never
       sits under the system clock on Android. -->
  <header
    class="fixed inset-x-0 top-0 z-50 flex items-center justify-between gap-space-md border-b border-outline-variant/40 bg-surface-container-lowest/90 backdrop-blur-xl"
    :style="{
      height: 'calc(var(--header-h) + var(--safe-top))',
      paddingTop: 'var(--safe-top)',
      paddingLeft: 'max(var(--spacing-space-md), var(--safe-left))',
      paddingRight: 'max(var(--spacing-space-md), var(--safe-right))',
    }"
  >
    <div class="flex min-w-0 items-center gap-space-md">
      <!-- Wordmark; the logo asset lives in resources/localdrop_logo. -->
      <div class="flex shrink-0 items-center gap-space-sm">
        <div
          class="flex h-8 w-8 items-center justify-center rounded-md bg-primary-container text-on-primary-container"
        >
          <svg viewBox="0 0 24 24" class="h-5 w-5" fill="none" aria-hidden="true">
            <path
              d="M12 3v11m0 0 4-4m-4 4-4-4"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
            <path
              d="M4 16v2a3 3 0 0 0 3 3h10a3 3 0 0 0 3-3v-2"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
            />
          </svg>
        </div>
        <div class="flex flex-col">
          <span
            class="font-headline-md text-headline-md font-semibold leading-none tracking-tight text-on-surface"
          >
            LocalDrop
          </span>
          <span class="font-label-sm text-label-sm uppercase tracking-widest text-primary">
            LAN direct
          </span>
        </div>
      </div>

      <div class="mx-space-xs hidden h-6 w-px bg-outline-variant/50 md:block" />

      <!--
        This device. Rendered only once `device` exists — the chip's own border
        and padding would otherwise draw an empty box during the first frames
        before hydration completes.
      -->
      <div
        v-if="device"
        class="hidden min-w-0 items-center gap-space-xs rounded-md border border-outline-variant/30 bg-surface-container-high/60 px-space-sm py-space-xs md:flex"
      >
        <OsIcon :os="device.os" :size="16" class="shrink-0 text-primary" />
        <span class="telemetry truncate text-on-surface">{{ device.alias }}</span>
        <span class="text-outline">•</span>
        <!-- Falls back to words, not a blank: FR-1.7's offline case has no IP. -->
        <span class="telemetry text-on-surface-variant">
          {{ network?.localIp ?? "no address" }}
        </span>
      </div>

      <!--
        Network scope. Three parallel selections on `peers.online` — icon, text
        colour, and label — which together satisfy UI-7: connection state is
        never carried by colour alone.
      -->
      <div
        class="hidden items-center gap-space-xs rounded-md bg-surface-container-low/80 px-space-sm py-space-xs xl:flex"
      >
        <component
          :is="peers.online ? Wifi : WifiOff"
          :size="14"
          :class="peers.online ? 'text-primary' : 'text-error'"
        />
        <span class="telemetry" :class="peers.online ? 'text-primary' : 'text-error'">
          <!--
            Nested fallback: online with a named interface shows the name,
            online without one shows "LAN" (a usable socket whose interface
            `if-addrs` could not label), and offline says so outright.
          -->
          {{ peers.online ? (network?.interface ?? "LAN") : "Offline" }}
        </span>
        <span class="text-outline">•</span>
        <!--
          The ports actually bound, not the defaults — §6.2's fallback can move
          them by up to 9, and a user comparing two devices needs the real ones.
        -->
        <span class="telemetry text-on-surface-variant">
          UDP {{ network?.discoveryPort ?? "—" }} / TCP {{ network?.transferPort ?? "—" }}
        </span>
      </div>
    </div>

    <div class="flex shrink-0 items-center gap-space-md">
      <!--
        §8.1 resolved as Option A: the UI states the real trust model instead of
        claiming encryption the v1.0 protocol does not implement. `encrypted`
        comes from the backend, so this label cannot drift from the code.

        Negated condition, deliberately: the warning appears when encryption is
        *absent*. Implementing Option B flips the backend's flag and this chip
        disappears on its own, with no template change.
      -->
      <div
        v-if="device && !device.encrypted"
        class="hidden items-center gap-space-xs rounded-full border border-outline-variant/30 bg-surface-container-low px-space-sm py-space-xs md:flex"
        title="v1.0 transfers are not encrypted. Use only on networks you trust."
      >
        <ShieldAlert :size="14" class="text-on-surface-variant" />
        <span class="font-label-sm text-label-sm text-on-surface-variant">
          Unencrypted · trusted LAN only
        </span>
      </div>

      <!--
        SEC-6 / FR-4.3 — auto-accept is never silently on. This is the one chip
        with no responsive `hidden` class: it must stay visible at every
        breakpoint, because a narrow window is not a reason to conceal that the
        device is accepting files from anyone on the network.
      -->
      <div
        v-if="settings.settings?.autoAccept"
        class="flex items-center gap-space-xs rounded-full bg-primary-container/20 px-space-sm py-space-xs"
      >
        <span class="h-2 w-2 animate-pulse rounded-full bg-primary" />
        <span class="font-label-sm text-label-sm font-semibold uppercase text-primary">
          Auto-accept on
        </span>
      </div>
    </div>
  </header>
</template>
