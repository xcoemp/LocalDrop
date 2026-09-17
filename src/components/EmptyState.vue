<!--
  EmptyState.vue — the shared "nothing here yet" panel (UI-4).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import type { Component } from "vue";

/** UI-4 — every list has a designed empty state, never a bare spinner. */
defineProps<{
  icon: Component;
  title: string;
  body?: string;
}>();
</script>

<template>
  <div
    class="flex flex-col items-center justify-center gap-space-sm rounded-lg border border-dashed border-outline-variant/50 bg-surface-container-lowest/60 px-space-lg py-space-xl text-center"
  >
    <component :is="icon" :size="28" class="text-outline" />
    <p class="font-headline-md text-headline-md text-on-surface">{{ title }}</p>

    <!--
      Selection: the body line is optional. Rendering the <p> unconditionally
      would leave an empty paragraph contributing its line-height to the gap,
      so callers that pass only a title would get visibly lopsided padding.
    -->
    <p v-if="body" class="max-w-sm font-body-md text-body-md text-on-surface-variant">
      {{ body }}
    </p>

    <!-- Optional call-to-action, e.g. Radar's "Rescan" button. -->
    <slot />
  </div>
</template>
