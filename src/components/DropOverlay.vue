<!--
  DropOverlay.vue — the full-window drag-and-drop affordance (UI-1).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { Crosshair, Download } from "lucide-vue-next";

/**
 * UI-1 — dragging files in dims the canvas and raises the peer cards. The
 * overlay is pointer-events-none so the cards underneath still resolve as hover
 * targets for `elementFromPoint`.
 *
 * Two independent booleans rather than one tri-state: `active` means a drag is
 * in progress anywhere over the window, `targeting` means the cursor is over a
 * specific peer card. They are not mutually exclusive, and the copy has to
 * distinguish "drop somewhere" from "drop here".
 */
defineProps<{ active: boolean; targeting: boolean }>();
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-150"
    leave-active-class="transition-opacity duration-150"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <!--
      Selection: mounted only while a drag is active. It must not merely be
      hidden with opacity, because a persistently mounted fixed overlay would
      keep intercepting the hit-testing that `useDragDrop` relies on.
    -->
    <div
      v-if="active"
      class="pointer-events-none fixed inset-0 z-40 flex items-end justify-center bg-background/60 backdrop-blur-[2px] pb-space-xl"
      aria-hidden="true"
    >
      <div
        class="flex items-center gap-space-md rounded-lg border border-primary/40 bg-surface-container px-space-lg py-space-md aura-ready"
      >
        <!--
          Three parallel selections on `targeting` — icon, heading, and body.
          Kept inline rather than hoisted into a computed pair because the
          template is the only consumer and splitting them across script and
          template would make the two states harder to read side by side.
        -->
        <component :is="targeting ? Crosshair : Download" :size="22" class="text-primary" />
        <div class="flex flex-col">
          <span class="font-headline-md text-headline-md text-on-surface">
            {{ targeting ? "Release to send" : "Drop on a device" }}
          </span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            {{
              targeting
                ? "This device will receive the files."
                : "Hover a peer card to choose where these go."
            }}
          </span>
        </div>
      </div>
    </div>
  </Transition>
</template>
