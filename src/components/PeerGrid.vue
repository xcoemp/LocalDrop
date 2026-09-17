<!--
  PeerGrid.vue — the radar's peer grid, its four empty states (UI-4), and the
  incompatible-version chips (FR-1.5).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { Radar, SearchX, TriangleAlert, WifiOff, X } from "lucide-vue-next";

import EmptyState from "@/components/EmptyState.vue";
import PeerCard from "@/components/PeerCard.vue";
import { usePeerStore } from "@/stores/usePeerStore";
import { useTransferStore } from "@/stores/useTransferStore";

const props = defineProps<{ hoverPeerId: string | null }>();

const emit = defineEmits<{
  sendFiles: [deviceId: string];
  sendText: [deviceId: string];
}>();

const peers = usePeerStore();
const transfers = useTransferStore();

const incompatibleList = computed(() => [...peers.incompatible.values()]);

// Distinguishes "no peers at all" from "no peers matching the filter" — the
// two need different empty states, and only the user's query separates them.
const filtering = computed(() => peers.filter.trim().length > 0);
</script>

<template>
  <div class="flex flex-col gap-space-md">
    <!--
      FR-1.5 — incompatible peers are chips, not cards: they are visible so the
      user understands why a known device is missing, but not connectable.
      The wrapper is conditional so its `gap` contributes nothing when empty.
    -->
    <div v-if="incompatibleList.length" class="flex flex-wrap gap-space-sm">
      <!-- Repetition over the incompatible map, keyed by device id so a peer
           re-broadcasting every 3 s updates its chip rather than adding one. -->
      <div
        v-for="peer in incompatibleList"
        :key="peer.deviceId"
        class="flex items-center gap-space-sm rounded-full border border-error/40 bg-error-container/20 py-1 pl-space-sm pr-1"
      >
        <TriangleAlert :size="14" class="text-error" />
        <span class="font-label-sm text-label-sm text-on-error-container">
          {{ peer.alias }} ({{ peer.ip }}) runs an incompatible version
        </span>
        <!-- FR-1.5 requires these be dismissible: the user may already know,
             and an undismissable warning would be permanent clutter. -->
        <button
          type="button"
          class="rounded-full p-1 text-on-error-container/70 hover:bg-error-container/40 hover:text-on-error-container"
          aria-label="Dismiss"
          @click="peers.dismissIncompatible(peer.deviceId)"
        >
          <X :size="12" />
        </button>
      </div>
    </div>

    <!--
      A four-way selection, ordered from the most fundamental problem to the
      least. Each branch answers a different user question, and the order is
      what makes them accurate: without a network there are necessarily no
      peers, so testing `online` first prevents "Looking for devices…" from
      appearing on a machine with the Wi-Fi off — a message that would be
      technically true and completely unhelpful.
    -->

    <!-- 1. FR-1.7 — no network is a stated condition, not an endless scan. -->
    <EmptyState
      v-if="!peers.online"
      :icon="WifiOff"
      title="No LAN connection"
      body="LocalDrop needs Wi-Fi or Ethernet. Connect to a network and discovery restarts automatically."
    />

    <!-- 2. Connected, but nobody has answered yet. Phrased as work in progress
            because that is what it is — a peer takes up to one heartbeat. -->
    <EmptyState
      v-else-if="!peers.list.length"
      :icon="Radar"
      title="Looking for devices…"
      body="Open LocalDrop on another device on this network. Peers usually appear within three seconds."
    />

    <!-- 3. Peers exist but the filter hides them all. `filtering` is required:
            without it this branch could never be reached, since case 2 already
            caught an empty list. -->
    <EmptyState
      v-else-if="!peers.visible.length && filtering"
      :icon="SearchX"
      title="No peers match that filter"
      :body="`Nothing matching “${peers.filter}”.`"
    />

    <!-- 4. The normal case. TransitionGroup so a peer pruned at its 12 s TTL
            fades rather than vanishing, which FR-1.3 requires not cause a
            layout jump. -->
    <TransitionGroup
      v-else
      name="peer"
      tag="div"
      class="relative grid grid-cols-1 gap-space-md sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4"
    >
      <!--
        Iterates `visible` (filtered), not `list`. Keyed by device id so a card
        keeps its DOM node across heartbeats — with an index key, a peer
        disappearing mid-list would make every card after it re-render and any
        in-flight progress bar animation would restart.
      -->
      <PeerCard
        v-for="peer in peers.visible"
        :key="peer.deviceId"
        :peer="peer"
        :selected="peers.selectedId === peer.deviceId"
        :drop-target="props.hoverPeerId === peer.deviceId"
        :transfer="transfers.forPeer(peer.deviceId)"
        @select="peers.select($event)"
        @send-files="emit('sendFiles', $event)"
        @send-text="emit('sendText', $event)"
      />
    </TransitionGroup>
  </div>
</template>
