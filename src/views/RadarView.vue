<!--
  RadarView.vue — the Discovery Radar screen: scanner telemetry, the peer
  filter, Rescan, and the peer grid (FR-1, §10.2).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { RefreshCcw, Search, X } from "lucide-vue-next";

import PeerGrid from "@/components/PeerGrid.vue";
import { usePeerStore } from "@/stores/usePeerStore";

defineProps<{ hoverPeerId: string | null }>();
const emit = defineEmits<{
  sendFiles: [deviceId: string];
  sendText: [deviceId: string];
}>();

const peers = usePeerStore();

/**
 * Derives `192.168.1.0/24`-style scope text from the interface snapshot.
 *
 * Computed here rather than in Rust because it is presentation only — the
 * backend already sends the IP and netmask that FR-6.4 requires, and CIDR is
 * just a compact way to render the pair.
 *
 * Degrades in two stages rather than failing, since this is a cosmetic label
 * that must never blank out the header it sits in.
 */
const subnet = computed(() => {
  const net = peers.network;

  // Stage 1: no address at all (offline, or before the first snapshot). A
  // generic phrase beats an empty space or a stray slash.
  if (!net?.localIp || !net.netmask) return "Local subnet";

  const ip = net.localIp.split(".").map(Number);
  const mask = net.netmask.split(".").map(Number);

  // Stage 2: not parseable as dotted-quad IPv4. Reached for an IPv6 address or
  // any unexpected format, where the bitwise arithmetic below would produce
  // nonsense. Showing the raw address is honest and still useful.
  if (ip.length !== 4 || mask.length !== 4 || ip.some(isNaN) || mask.some(isNaN)) {
    return net.localIp;
  }

  // Network address: each octet ANDed with its mask octet.
  const network = ip.map((octet, i) => octet & mask[i]).join(".");

  // Prefix length: count the set bits across the mask. Folding over the four
  // octets and measuring each one's binary string with the zeros stripped is a
  // compact population count — `toString(2)` omits leading zeros, so what
  // remains after removing every "0" is exactly the number of ones.
  const bits = mask.reduce((total, octet) => total + octet.toString(2).replace(/0/g, "").length, 0);
  return `${network}/${bits}`;
});
</script>

<template>
  <div class="flex flex-col gap-space-lg">
    <!-- Scanner telemetry bar (discovery_radar_peer_grid mockup) -->
    <section
      class="flex w-full flex-wrap items-center justify-between gap-space-md rounded-lg bg-surface-container-lowest p-space-md"
    >
      <div class="flex min-w-0 items-center gap-space-md">
        <div
          class="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-surface-container-high text-primary"
        >
          <!-- Spins only while a rescan is running (FR-1.6); static otherwise,
               so perpetual motion does not imply perpetual work. -->
          <RefreshCcw :size="22" :class="peers.scanning ? 'animate-spin' : ''" />
        </div>
        <div class="flex min-w-0 flex-col">
          <div class="flex items-center gap-space-xs">
            <span class="truncate font-headline-md text-headline-md text-on-surface">
              Subnet {{ subnet }}
            </span>
            <!-- The live-discovery pulse. Suppressed when offline: an animated
                 "listening" indicator beside a dead socket would be a lie. -->
            <span v-if="peers.online" class="relative flex h-2 w-2">
              <span
                class="absolute inline-flex h-full w-full animate-ping rounded-full bg-primary opacity-75"
              />
              <span class="relative inline-flex h-2 w-2 rounded-full bg-primary" />
            </span>
          </div>
          <div class="telemetry flex flex-wrap items-center gap-space-sm text-on-surface-variant">
            <!-- Socket state in words, since this line is the first thing to
                 check when no peers appear (see docs/BUILDING.md's checklist). -->
            <span
              >UDP broadcast {{ peers.network?.discoveryPort ?? "—" }}:
              {{ peers.online ? "BIND OK" : "DOWN" }}</span
            >
            <span class="text-outline-variant">•</span>
            <span>TCP {{ peers.network?.transferPort ?? "—" }}</span>
            <span class="text-outline-variant">•</span>
            <span class="text-primary">Heartbeat 3s</span>
          </div>
        </div>
      </div>

      <div class="flex flex-grow items-center gap-space-sm md:flex-grow-0">
        <div class="relative flex-1 md:w-64">
          <Search
            :size="18"
            class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-on-surface-variant"
          />
          <!--
            Bound manually rather than with `v-model`, for the same IME reason
            as the snippet composer: Android's predictive keyboard holds a word
            in composition, and `v-model` ignores input until it commits — so
            filtering would not begin until the user typed a space.
          -->
          <input
            :value="peers.filter"
            @input="peers.filter = ($event.target as HTMLInputElement).value"
            @compositionupdate="peers.filter = ($event.target as HTMLInputElement).value"
            type="text"
            placeholder="Filter host, OS, or IP…"
            aria-label="Filter peers"
            class="w-full rounded-md bg-surface-container-low py-1.5 pl-9 pr-8 font-label-md text-label-md text-on-surface placeholder:text-outline focus:outline-none focus:ring-1 focus:ring-primary"
          />
          <!-- Clear button appears only once there is something to clear, so
               the input is not permanently crowded by a dead affordance. -->
          <button
            v-if="peers.filter"
            type="button"
            class="absolute right-2.5 top-1/2 -translate-y-1/2 text-outline hover:text-on-surface"
            aria-label="Clear filter"
            @click="peers.filter = ''"
          >
            <X :size="16" />
          </button>
        </div>

        <button
          type="button"
          class="flex items-center gap-space-xs rounded-md bg-primary-container px-space-md py-1.5 font-label-md text-label-md font-semibold text-on-primary-container transition-all hover:opacity-90 active:scale-95 disabled:opacity-50 aura-ready"
          :disabled="peers.scanning"
          @click="peers.rescan()"
        >
          <RefreshCcw :size="18" :class="peers.scanning ? 'animate-spin' : ''" />
          <span>Rescan</span>
        </button>
      </div>
    </section>

    <div class="flex items-center justify-between px-space-xs">
      <div class="flex items-center gap-space-xs">
        <span
          class="font-label-sm text-label-sm font-semibold uppercase tracking-widest text-outline"
        >
          Discovered peers
        </span>
        <!-- `list`, not `visible`: this is the true peer count, and it should
             not drop as the user narrows the filter. -->
        <span
          class="telemetry rounded-full bg-surface-container-high px-space-xs font-bold text-primary"
        >
          {{ peers.list.length }} ONLINE
        </span>
      </div>
      <!-- Legend for the status dots (UI-7). Hidden on the narrowest screens,
           where each card's own text label carries the same information. -->
      <div class="telemetry hidden items-center gap-space-sm text-on-surface-variant sm:flex">
        <span class="flex items-center gap-1">
          <span class="h-1.5 w-1.5 rounded-full bg-primary" />Ready
        </span>
        <span class="flex items-center gap-1">
          <span class="h-1.5 w-1.5 rounded-full bg-tertiary" />Transferring
        </span>
      </div>
    </div>

    <PeerGrid
      :hover-peer-id="hoverPeerId"
      @send-files="emit('sendFiles', $event)"
      @send-text="emit('sendText', $event)"
    />
  </div>
</template>
