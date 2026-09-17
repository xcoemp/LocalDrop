<!--
  App.vue — application shell: header, navigation, view switching, and the
  globally-mounted dialogs and overlays.

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { platform } from "@tauri-apps/plugin-os";

import DropOverlay from "@/components/DropOverlay.vue";
import IncomingOfferDialog from "@/components/IncomingOfferDialog.vue";
import PeerPickerDialog from "@/components/PeerPickerDialog.vue";
import SendTextModal from "@/components/SendTextModal.vue";
import TheHeader from "@/components/TheHeader.vue";
import TheSidebar, { type ViewId } from "@/components/TheSidebar.vue";
import ToastStack from "@/components/ToastStack.vue";
import TransferCenter from "@/components/TransferCenter.vue";
import RadarView from "@/views/RadarView.vue";
import SendView from "@/views/SendView.vue";
import SettingsView from "@/views/SettingsView.vue";

import { useDragDrop } from "@/composables/useDragDrop";
import { useTauriEvents } from "@/composables/useTauriEvents";
import { usePeerStore } from "@/stores/usePeerStore";
import { useSettingsStore } from "@/stores/useSettingsStore";
import { useTransferStore } from "@/stores/useTransferStore";

const peers = usePeerStore();
const transfers = useTransferStore();
const settings = useSettingsStore();

// Local state, not a router: four screens with no deep-linkable URLs and no
// browser history to integrate with. A router would add a dependency and a
// bundle for navigation the user performs with four buttons.
const view = ref<ViewId>("radar");
const textModalOpen = ref(false);
const textTarget = ref<string | null>(null);

// Mounted once, here, for the whole app — see useTauriEvents for why.
useTauriEvents();
const { dragging, hoverPeerId, pendingPaths, resolvePending, cancelPending } = useDragDrop();

function openTextModal(deviceId: string) {
  textTarget.value = deviceId;
  // Selecting as well as targeting keeps the radar's highlight in agreement
  // with what the modal is about to send to.
  peers.select(deviceId);
  textModalOpen.value = true;
}

/** Jump straight to the send workspace with this peer pre-selected. */
function startFileSend(deviceId: string) {
  peers.select(deviceId);
  view.value = "send";
}

onMounted(async () => {
  // Seed from the backend, then let the event stream keep things current.
  // Parallel because the three are independent and the §9 cold-start budget is
  // 1.5 s on Windows — three sequential IPC round-trips would eat into it.
  await Promise.all([settings.hydrate(), peers.refresh(), transfers.hydrate()]);

  // AND-2 — start the foreground service once the UI is up, rather than during
  // Rust setup: the JNI bridge needs the Android activity to exist, and at
  // startup it may not yet. No-op on desktop.
  //
  // Both conditions matter: the platform check keeps the command off desktop
  // (where it is meaningless), and the setting check honours a user who turned
  // background operation off and does not want the permanent notification.
  if (platform() === "android" && settings.settings?.backgroundService) {
    try {
      await invoke("start_background_listener");
    } catch {
      // Losing background protection must not stop the app from running.
    }
  }

  // UI-6 — paste sends the clipboard to the selected peer.
  window.addEventListener("keydown", (event) => {
    // Three conditions, all required: the modifier (either platform's), the
    // key, and a destination to send to. Without the last, the shortcut would
    // open a modal with nowhere to send.
    if ((event.ctrlKey || event.metaKey) && event.key === "v" && peers.selectedId) {
      // Bail out when the user is typing: intercepting Ctrl+V inside the
      // snippet composer or the alias field would make those fields unpasteable.
      const active = document.activeElement;
      const editing = active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement;
      if (editing) return;

      // Only prevented once we are certain we are handling it, so a paste we
      // decline to intercept still reaches the focused element.
      event.preventDefault();
      openTextModal(peers.selectedId);
    }
  });
});
</script>

<template>
  <div class="min-h-screen bg-background text-on-surface">
    <TheHeader />
    <TheSidebar v-model="view" />

    <!-- Top clears the header + status bar; bottom clears the mobile nav and
         gesture bar (UI-5). See `.app-main` in style.css — the desktop rail has
         no bottom bar, so the reservation is zeroed there. -->
    <main class="app-main min-h-screen lg:pl-72">
      <div class="app-gutter flex w-full flex-col gap-space-lg">
        <!--
          Selection: the four screens, as a v-if/v-else-if chain on `view`.
          Chained rather than four independent `v-if`s so exactly one can ever
          be mounted, and mounted rather than merely shown so each view runs its
          `onMounted` work — SettingsView polls background status, SendView
          resets its queue — on every visit rather than once per session.

          SettingsView is the bare `v-else` so an unrecognised `view` value
          still renders something navigable rather than an empty shell.
        -->
        <RadarView
          v-if="view === 'radar'"
          :hover-peer-id="hoverPeerId"
          @send-files="startFileSend"
          @send-text="openTextModal"
        />
        <SendView v-else-if="view === 'send'" @send-text="openTextModal" />
        <TransferCenter v-else-if="view === 'transfers'" />
        <SettingsView v-else />
      </div>
    </main>

    <!--
      Mounted at the shell level rather than inside a view, because all four
      must be reachable regardless of which screen is open: an offer can arrive
      (FR-4.1) and a drop can land (UI-1) while the user is in Settings.
      Each manages its own visibility internally.
    -->
    <DropOverlay :active="dragging" :targeting="!!hoverPeerId" />

    <PeerPickerDialog :paths="pendingPaths" @pick="resolvePending" @cancel="cancelPending" />

    <SendTextModal :open="textModalOpen" :device-id="textTarget" @close="textModalOpen = false" />

    <IncomingOfferDialog />
    <ToastStack />
  </div>
</template>
