/**
 * useSettingsStore.ts — settings and device identity (PRD FR-6).
 *
 * Every mutation round-trips through Rust, which normalizes and persists it
 * (FR-6.5), then returns the canonical value — so the UI can never drift from
 * what is actually on disk.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

import type { DeviceInfo, Settings } from "@/types/protocol";

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<Settings | null>(null);
  const saving = ref(false);
  const savedAt = ref<number | null>(null);

  /**
   * `get_device_info` as Rust returned it. Read through `device` instead —
   * this is only the immutable half.
   */
  const identity = ref<DeviceInfo | null>(null);

  /**
   * FR-6.1 — `alias` is the one field of `DeviceInfo` the user can change while
   * the app is running, and Rust derives it from `deviceAlias` anyway. Deriving
   * it here rather than caching Rust's copy means every consumer (the header
   * chip above all) updates the moment Settings commits, and there is no second
   * copy left to go stale.
   *
   * During the optimistic window in `update` this briefly shows the raw input
   * rather than Rust's normalized value — which is the point: the name the user
   * just typed appears immediately, and self-corrects if the backend clamps it.
   */
  const device = computed<DeviceInfo | null>(() => {
    // Nothing to overlay before the first hydrate resolves. Callers already
    // handle null (the header hides its chip), so returning a partial object
    // would be worse than returning none.
    if (!identity.value) return null;

    // `??` rather than a plain read: settings and identity hydrate together,
    // but this computed can be evaluated between the two assignments, and
    // falling back to Rust's copy keeps the alias correct in that window.
    return { ...identity.value, alias: settings.value?.deviceAlias ?? identity.value.alias };
  });

  async function hydrate() {
    const [loaded, info] = await Promise.all([
      invoke<Settings>("get_settings"),
      invoke<DeviceInfo>("get_device_info"),
    ]);
    settings.value = loaded;
    identity.value = info;
  }

  async function update(patch: Partial<Settings>) {
    // Guard: a patch applied before hydration would send a half-built Settings
    // object to Rust, which would then persist the missing fields as defaults
    // and silently reset everything the user had configured.
    if (!settings.value) return;

    const next: Settings = { ...settings.value, ...patch };
    // Optimistic so toggles feel instant; Rust's normalized reply is authoritative.
    settings.value = next;
    saving.value = true;
    try {
      settings.value = await invoke<Settings>("update_settings", { settings: next });
      savedAt.value = Date.now();
    } finally {
      // `finally`, so a rejected write still clears the saving indicator. Note
      // the optimistic value stays in place on failure — the alternative is
      // reverting a toggle under the user's cursor, and Rust's only failure
      // mode here is a disk write, which the next change retries anyway.
      saving.value = false;
    }
  }

  function apply(next: Settings) {
    settings.value = next;
  }

  async function factoryReset() {
    saving.value = true;
    try {
      settings.value = await invoke<Settings>("factory_reset");
      // Re-fetched because a reset issues a new device_id, which `device`
      // cannot derive from settings.
      identity.value = await invoke<DeviceInfo>("get_device_info");
      savedAt.value = Date.now();
    } finally {
      saving.value = false;
    }
  }

  return { settings, device, saving, savedAt, hydrate, update, apply, factoryReset };
});
