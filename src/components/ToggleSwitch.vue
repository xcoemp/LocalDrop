<!--
  ToggleSwitch.vue — the pill toggle used by the settings panel and sidebar.

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
/**
 * The pill toggle from the mockups' sidebar and settings panel.
 *
 * UI-7 — state is conveyed by position and an accessible label, not by color
 * alone, and the control is a real button so it is keyboard reachable.
 */
const model = defineModel<boolean>({ required: true });

defineProps<{ label: string; disabled?: boolean }>();
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="model"
    :aria-label="label"
    :disabled="disabled"
    class="relative flex h-5 w-10 shrink-0 items-center rounded-full p-0.5 transition-colors disabled:cursor-not-allowed disabled:opacity-40"
    :class="model ? 'bg-primary-container' : 'bg-surface-container-highest'"
    @click="model = !model"
  >
    <!--
      Selection: the knob's colour and offset both key off `model`. Two
      independent cues (position and fill) rather than colour alone, per UI-7.
    -->
    <span
      class="h-4 w-4 rounded-full transition-transform"
      :class="model ? 'translate-x-5 bg-on-primary-container' : 'translate-x-0 bg-outline'"
    />
  </button>
</template>
