<!--
  OsIcon.vue — maps a peer's reported OS to an icon and a text label (§10.2).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed } from "vue";
import { HelpCircle, Laptop, MonitorSmartphone, Smartphone, Terminal } from "lucide-vue-next";

import type { PeerOs } from "@/types/protocol";

const props = withDefaults(defineProps<{ os: PeerOs; size?: number }>(), {
  size: 22,
});

/**
 * §10.2 — every peer card carries an OS badge.
 *
 * Selection over the `os` enum. The `default` arm is load-bearing rather than
 * defensive padding: `os` arrives from a peer's heartbeat over the network, so
 * a future or malformed value is entirely possible and must degrade to a
 * neutral icon instead of rendering nothing.
 */
const icon = computed(() => {
  switch (props.os) {
    case "android":
      return Smartphone;
    case "windows":
      return MonitorSmartphone;
    case "macos":
      // macOS and Linux are unshipped targets (§2.2) but the protocol already
      // admits them, so a peer may legitimately report either.
      return Laptop;
    case "linux":
      return Terminal;
    default:
      return HelpCircle;
  }
});

/**
 * The same selection again, for the accessible name.
 *
 * UI-7 requires status never be conveyed by icon or colour alone, so the label
 * is exposed both as `aria-label` here and as text by the peer card. Kept as a
 * separate `switch` rather than a shared lookup table so that adding an OS
 * cannot add an icon without also adding its label — the compiler flags the
 * missing arm in whichever one is forgotten.
 */
const label = computed(() => {
  switch (props.os) {
    case "android":
      return "Android";
    case "windows":
      return "Windows";
    case "macos":
      return "macOS";
    case "linux":
      return "Linux";
    default:
      return "Unknown";
  }
});

defineExpose({ label });
</script>

<template>
  <component :is="icon" :size="size" :aria-label="label" />
</template>
