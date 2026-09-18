<!--
  SettingsView.vue — device identity, storage destination, automation toggles,
  diagnostics, and factory reset (FR-6, §10.2).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { platform } from "@tauri-apps/plugin-os";
import { BatteryCharging, FolderOpen, RotateCcw, ShieldAlert } from "lucide-vue-next";

import FileBrowser from "@/components/FileBrowser.vue";
import ToggleSwitch from "@/components/ToggleSwitch.vue";
import { pickerStartDir } from "@/composables/usePickerDir";
import { usePeerStore } from "@/stores/usePeerStore";
import { useSettingsStore } from "@/stores/useSettingsStore";
import { useTransferStore } from "@/stores/useTransferStore";
import type { BackgroundStatus, CollisionPolicy, Settings } from "@/types/protocol";

const settings = useSettingsStore();
const peers = usePeerStore();
const transfers = useTransferStore();

// Computed once at setup, not reactive: the platform cannot change at runtime.
// Gates the Android-only and desktop-only rows below.
const isAndroid = platform() === "android";
const confirmingReset = ref(false);

/**
 * Two-way binding that persists through Rust on every change (FR-6.5).
 *
 * A factory that builds one writable computed per key, rather than six
 * near-identical hand-written pairs. Every toggle behaves the same — read the
 * store, write through `update` — so the repetition is better expressed once.
 * The generic keeps each binding's type tied to its own field.
 */
function field<K extends keyof Settings>(key: K) {
  return computed({
    // The cast is needed because `settings` may be null before hydration;
    // `undefined` binds harmlessly to an unmounted toggle for those few frames.
    get: () => settings.settings?.[key] as Settings[K],
    set: (value: Settings[K]) => void settings.update({ [key]: value } as Partial<Settings>),
  });
}

const autoAccept = field("autoAccept");
const autoCopySnippets = field("autoCopySnippets");
const completionSound = field("completionSound");
const transferStartSound = field("transferStartSound");
const launchMinimized = field("launchMinimized");
const organizeBySender = field("organizeBySender");
const organizeByFileType = field("organizeByFileType");

/**
 * Local draft so the field is editable mid-typing without persisting every
 * keystroke. Kept in step with the store, which changes under it on hydration,
 * when Rust clamps the alias to 32 chars (FR-6.1), and after a factory reset —
 * typing does not touch the store, so this can never fight the user.
 */
const alias = ref(settings.settings?.deviceAlias ?? "");

watch(
  () => settings.settings?.deviceAlias,
  (next) => {
    // `undefined` means the store has not hydrated. Assigning it would blank
    // the input; leaving the draft alone is correct in that window.
    if (next !== undefined) alias.value = next;
  },
  // `immediate` so a view mounted after hydration still picks up the value,
  // which the initialiser above would have missed.
  { immediate: true },
);

/** Persist on blur or Enter, not per keystroke — see the draft note above. */
function commitAlias() {
  const next = alias.value.trim();

  // Two no-op cases in one branch: an empty field (Rust would substitute the
  // hostname, which is not what an empty box implies) and an unchanged value
  // (which would cost a disk write and a heartbeat for nothing).
  if (!next || next === settings.settings?.deviceAlias) {
    // Restore the canonical value, so a cleared or unchanged field does not sit
    // there showing something the app is not broadcasting.
    alias.value = settings.settings?.deviceAlias ?? alias.value;
    return;
  }
  void settings.update({ deviceAlias: next });
}

/**
 * FR-6.2 / AND-4 — always a picker, never a text field. Under Android scoped
 * storage an arbitrary typed path is not writable.
 */
const browserOpen = ref(false);

async function pickDownloadDir() {
  // Android: the SAF folder picker returns a tree URI, and folders outside
  // device storage cannot be written at all. Browsing real paths is both
  // simpler and honest about what is reachable.
  if (isAndroid) {
    browserOpen.value = true;
    return;
  }
  const picked = await open({
    directory: true,
    multiple: false,
    title: "Choose download folder",
    // Starts at the folder currently in use rather than the app's install
    // directory, so "change this slightly" does not mean navigating from
    // scratch. Falls back to Downloads when the configured path has since been
    // deleted, which `open` would otherwise treat as no preference at all.
    defaultPath: settings.settings?.downloadDirectory || (await pickerStartDir()),
  });
  // Cancelled, or an unexpected array; either way there is nothing to apply.
  if (!picked || Array.isArray(picked)) return;
  await applyDownloadDir(picked);
}

async function applyDownloadDir(picked: string) {
  // Goes through Rust rather than update(): on Android the picker returns a SAF
  // tree URI, which has to be resolved to a real path and proven writable
  // before it is stored. Saving it blind would fail later, mid-transfer.
  try {
    // `apply`, not `update`: Rust has already normalised and persisted this, so
    // writing it back through `update` would trigger a redundant second save.
    settings.apply(await invoke<Settings>("set_download_directory", { path: picked }));
    transfers.toast({ kind: "success", title: "Download folder updated" });
    // Closed only on success, so a rejected folder leaves the browser open at
    // the same place for the user to pick a different one.
    browserOpen.value = false;
  } catch (error) {
    // FR-6.2's writability check failing is the expected error here, and it is
    // worth the round trip: finding out now beats finding out mid-transfer.
    const payload = error as { message?: string; code?: string };
    transfers.toast({
      kind: "error",
      title: payload.message ?? "Couldn't use that folder.",
      code: payload.code,
    });
  }
}

/**
 * AND-2 — the foreground service that keeps sockets alive in the background.
 *
 * Toggled through dedicated commands rather than `update()` because each side
 * has to actually start or stop the native service, not just record a flag.
 */
const background = ref<BackgroundStatus | null>(null);

async function refreshBackground() {
  // Guarded because the command exists only on Android; calling it on desktop
  // would reject, and the whole row it feeds is hidden there anyway.
  if (!isAndroid) return;
  background.value = await invoke<BackgroundStatus>("get_background_status");
}

/**
 * The background-service switch.
 *
 * Written as a computed with a side-effecting setter so it can bind to
 * `ToggleSwitch` like any other setting, while the two directions actually call
 * different commands. The setter is deliberately not `async` — a `v-model`
 * setter cannot be awaited — so the work runs in a detached IIFE and the state
 * is refreshed from whatever the command returns.
 */
const backgroundEnabled = computed({
  get: () => background.value?.enabled ?? false,
  set: (value: boolean) => {
    void (async () => {
      // Selection on the new value, since starting and stopping the native
      // service are separate operations rather than one parameterised call.
      const command = value ? "start_background_listener" : "stop_background_listener";
      try {
        background.value = await invoke<BackgroundStatus>(command);
      } catch {
        // Not updating `background` on failure is what makes the switch snap
        // back — the UI keeps reflecting the service's real state, not the
        // state the user asked for.
        transfers.toast({ kind: "error", title: "Couldn't change background mode." });
      }
    })();
  },
});

async function requestBatteryExemption() {
  await invoke("request_battery_exemption");
  // The prompt is a separate activity; re-check when the user comes back.
  // A delay rather than a resume listener: the answer is only known once the
  // system dialog has closed, and one second covers the round trip.
  setTimeout(() => void refreshBackground(), 1000);
}

onMounted(refreshBackground);

/**
 * FR-2.8's collision policy. Defaults to `rename` in the getter, matching the
 * schema, so the three-way selector never renders with nothing highlighted.
 */
const collisionPolicy = computed({
  get: () => settings.settings?.collisionPolicy ?? "rename",
  set: (value: CollisionPolicy) => void settings.update({ collisionPolicy: value }),
});

async function reset() {
  // The alias field follows the store via its watcher; nothing to reset here.
  await settings.factoryReset();
  // Collapses the confirmation step back to a single button.
  confirmingReset.value = false;
  transfers.toast({ kind: "success", title: "Restored default settings" });
}
</script>

<template>
  <!--
    The whole view waits on hydration. Unlike the other screens, which degrade
    to empty states, a settings panel rendered from nulls would briefly show
    every toggle off and an empty alias — indistinguishable from a real
    configuration, and alarming if the user happens to look.
  -->
  <div v-if="settings.settings" class="grid grid-cols-1 items-start gap-space-lg xl:grid-cols-2">
    <!-- Device identity -->
    <section class="flex flex-col gap-space-md rounded-lg bg-surface-container-low p-space-lg">
      <h2 class="font-headline-md text-headline-md text-on-surface">
        Device identity & LAN presence
      </h2>

      <label class="flex flex-col gap-space-xs">
        <span class="font-label-md text-label-md text-on-surface-variant">
          Broadcast device name
        </span>
        <!--
          `v-model` is safe here, unlike the IME-sensitive fields elsewhere:
          this commits on blur or Enter rather than reacting per keystroke, so a
          word held in composition is committed by the time it matters.

          `maxlength` mirrors FR-6.1's 32-character cap client-side, so the
          limit is felt while typing; Rust clamps it again authoritatively.
        -->
        <input
          v-model="alias"
          type="text"
          maxlength="32"
          class="rounded-md border border-outline-variant/40 bg-surface-container-lowest px-space-md py-space-sm font-body-md text-body-md text-on-surface focus:border-secondary-container focus:outline-none"
          @blur="commitAlias"
          @keydown.enter="commitAlias"
        />
        <span class="telemetry text-outline">
          Shown to nearby devices. Updates within one heartbeat (3s).
        </span>
      </label>

      <div class="flex flex-col gap-space-xs">
        <span class="font-label-md text-label-md text-on-surface-variant">
          Default download directory
        </span>
        <!--
          Stacks below sm, side by side above it.

          Sharing one row with Browse left the path perhaps 180px on a phone,
          which truncated even a short "C:\Users\User\Downloads\LocalDrop" —
          and this is the one screen whose whole purpose is to tell the user
          where their files go. Dropping the button onto its own line gives the
          path the full width, and the path then wraps to as many lines as it
          needs rather than being cut.
        -->
        <div class="flex flex-col items-start gap-space-sm sm:flex-row sm:items-center">
          <!--
            `w-full` *and* `sm:flex-1`, which are not redundant: in the stacked
            column layout `flex-1` would grow the box vertically, since it acts
            on the main axis — only `w-full` makes it span the row. Above sm the
            main axis is horizontal and `flex-1` is what claims the space Browse
            leaves over.
          -->
          <span
            data-selectable
            class="w-full min-w-0 wrap-anywhere rounded-md bg-surface-container-lowest px-space-md py-space-sm font-body-md text-body-md text-on-surface sm:flex-1"
            :title="settings.settings.downloadDirectory"
          >
            {{ settings.settings.downloadDirectory }}
          </span>
          <!-- `shrink-0` so the button keeps its label intact once it is back
               on the same row as the path above sm. -->
          <button
            type="button"
            class="flex shrink-0 items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface hover:bg-surface-bright"
            @click="pickDownloadDir"
          >
            <FolderOpen :size="16" />
            <span>Browse…</span>
          </button>
        </div>
      </div>
    </section>

    <!-- Automation -->
    <section class="flex flex-col gap-space-md rounded-lg bg-surface-container-low p-space-lg">
      <h2 class="font-headline-md text-headline-md text-on-surface">Transfer automation</h2>

      <div class="flex items-start justify-between gap-space-md">
        <div class="flex flex-col">
          <span class="font-label-md text-label-md text-on-surface">
            Auto-accept incoming transfers
          </span>
          <!-- SEC-6 — the risk is stated where the switch is, not in a footnote. -->
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            Files arrive without asking. Anyone on this network can send to you.
          </span>
        </div>
        <ToggleSwitch v-model="autoAccept" label="Auto-accept incoming transfers" />
      </div>

      <div class="flex items-start justify-between gap-space-md">
        <div class="flex flex-col">
          <span class="font-label-md text-label-md text-on-surface">Auto-copy text snippets</span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            Received snippets go straight to the clipboard.
          </span>
        </div>
        <ToggleSwitch v-model="autoCopySnippets" label="Auto-copy text snippets" />
      </div>

      <div class="flex items-start justify-between gap-space-md">
        <div class="flex flex-col">
          <span class="font-label-md text-label-md text-on-surface">Play completion sound</span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            A short chime when a transfer finishes.
          </span>
        </div>
        <ToggleSwitch v-model="completionSound" label="Play completion sound" />
      </div>

      <!-- Placed directly after the completion sound, since the two are the
           same kind of choice and are most easily understood as a pair. -->
      <div class="flex items-start justify-between gap-space-md">
        <div class="flex flex-col">
          <span class="font-label-md text-label-md text-on-surface">Play start sound</span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            A lower tone when a transfer begins. Useful with auto-accept on, where files otherwise
            start arriving silently.
          </span>
        </div>
        <ToggleSwitch v-model="transferStartSound" label="Play start sound" />
      </div>

      <!-- AND-2: Android only; desktop processes are not suspended. -->
      <div v-if="isAndroid" class="flex items-start justify-between gap-space-md">
        <div class="flex flex-col">
          <span class="font-label-md text-label-md text-on-surface">
            Keep running in the background
          </span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            Stays reachable and keeps transfers alive when you switch apps. Shows a permanent
            notification, which Android requires.
          </span>
        </div>
        <ToggleSwitch v-model="backgroundEnabled" label="Keep running in the background" />
      </div>

      <!-- Doze can freeze a non-exempt app between maintenance windows, which
           strands a long transfer even with the service running. -->
      <div
        v-if="isAndroid && background && !background.batteryUnrestricted"
        class="flex items-start justify-between gap-space-md rounded-md border border-outline-variant/40 bg-surface-container-lowest p-space-md"
      >
        <div class="flex min-w-0 flex-col">
          <span class="font-label-md text-label-md text-on-surface">
            Battery optimisation is on
          </span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            Android may still pause long transfers. Allowing unrestricted background use prevents
            that.
          </span>
        </div>
        <button
          type="button"
          class="flex shrink-0 items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface hover:bg-surface-bright"
          @click="requestBatteryExemption"
        >
          <BatteryCharging :size="16" />
          <span>Allow</span>
        </button>
      </div>

      <!-- The desktop counterpart to the Android row above: §7.1's tray
           behaviour has no meaning on a phone, so the two are mutually
           exclusive rather than both always present. -->
      <div v-if="!isAndroid" class="flex items-start justify-between gap-space-md">
        <div class="flex flex-col">
          <span class="font-label-md text-label-md text-on-surface">
            Launch minimized to system tray
          </span>
          <span class="font-body-sm text-body-sm text-on-surface-variant">
            Start receiving without opening the window.
          </span>
        </div>
        <ToggleSwitch v-model="launchMinimized" label="Launch minimized to tray" />
      </div>
    </section>

    <!-- Folder organization + collisions -->
    <section class="flex flex-col gap-space-md rounded-lg bg-surface-container-low p-space-lg">
      <h2 class="font-headline-md text-headline-md text-on-surface">Folder organization</h2>

      <div class="flex items-center justify-between gap-space-md">
        <span class="font-label-md text-label-md text-on-surface">Organize by sender hostname</span>
        <ToggleSwitch v-model="organizeBySender" label="Organize by sender hostname" />
      </div>
      <div class="flex items-center justify-between gap-space-md">
        <span class="font-label-md text-label-md text-on-surface">Organize by file extension</span>
        <ToggleSwitch v-model="organizeByFileType" label="Organize by file extension" />
      </div>

      <div class="flex flex-col gap-space-xs">
        <span class="font-label-md text-label-md text-on-surface-variant">
          When a file already exists
        </span>
        <!--
          FR-2.8's three policies as a segmented control. Iterated from an
          inline literal rather than three buttons: the array order is also the
          display order, and the active styling is then written once. `rename`
          is first because it is the default and the non-destructive choice.
        -->
        <div class="flex gap-space-xs">
          <button
            v-for="option in ['rename', 'overwrite', 'skip'] as CollisionPolicy[]"
            :key="option"
            type="button"
            class="flex-1 rounded-md px-space-md py-space-sm font-label-md text-label-md capitalize transition-colors"
            :class="
              collisionPolicy === option
                ? 'bg-primary-container font-semibold text-on-primary-container'
                : 'bg-surface-container-high text-on-surface-variant hover:text-on-surface'
            "
            @click="collisionPolicy = option"
          >
            {{ option }}
          </button>
        </div>
      </div>
    </section>

    <!-- Diagnostics (FR-6.4) -->
    <section
      class="flex flex-col gap-space-md rounded-lg bg-surface-container-low p-space-lg xl:col-span-2"
    >
      <h2 class="font-headline-md text-headline-md text-on-surface">Network & diagnostics</h2>

      <!--
        FR-6.4's read-only panel. Every value falls back to an em-dash rather
        than rendering blank, so a missing reading is visibly missing — this is
        the panel a user is asked to read out when discovery is not working, and
        an empty row is ambiguous between "none" and "not loaded".

        Note the gateway row FR-6.4 asks for is absent entirely: `if-addrs` does
        not expose it (PRD §15.1).
      -->
      <dl class="grid grid-cols-1 gap-space-sm sm:grid-cols-2 2xl:grid-cols-4">
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Active interface</dt>
          <dd class="telemetry text-on-surface">{{ peers.network?.interface ?? "—" }}</dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Local IP</dt>
          <dd class="telemetry text-on-surface">{{ peers.network?.localIp ?? "—" }}</dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Subnet mask</dt>
          <dd class="telemetry text-on-surface">{{ peers.network?.netmask ?? "—" }}</dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Broadcast target</dt>
          <!-- First target only: §6.2 computes one per usable interface, and
               the primary is the diagnostically interesting one. -->
          <dd class="telemetry text-on-surface">
            {{ peers.network?.broadcastTargets[0] ?? "—" }}
          </dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Discovery port</dt>
          <dd class="telemetry text-primary">UDP {{ peers.network?.discoveryPort ?? "—" }}</dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Transfer port</dt>
          <dd class="telemetry text-primary">TCP {{ peers.network?.transferPort ?? "—" }}</dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Device ID</dt>
          <!-- `data-selectable` so the UUID can be copied when comparing two
               devices; the app suppresses text selection everywhere else. -->
          <dd data-selectable class="telemetry truncate text-on-surface-variant">
            {{ settings.device?.deviceId ?? "—" }}
          </dd>
        </div>
        <div
          class="flex justify-between gap-space-sm rounded-md bg-surface-container-lowest p-space-sm"
        >
          <dt class="font-label-sm text-label-sm text-on-surface-variant">Protocol</dt>
          <dd class="telemetry text-on-surface">
            v{{ settings.device?.protocolVersion }} · app {{ settings.device?.clientVersion }}
          </dd>
        </div>
      </dl>

      <!-- §8 — the trust model, stated plainly. -->
      <div
        class="flex items-start gap-space-sm rounded-md border border-outline-variant/40 bg-surface-container-lowest p-space-md"
      >
        <ShieldAlert :size="18" class="mt-0.5 shrink-0 text-on-surface-variant" />
        <p class="font-body-sm text-body-sm text-on-surface-variant">
          Transfers are sent unencrypted over your local network and are not authenticated. Any
          device on this network can discover this one and offer it files. Use LocalDrop on networks
          you trust.
        </p>
      </div>
    </section>

    <!-- Factory reset -->
    <section
      class="flex items-center justify-between gap-space-md rounded-lg bg-surface-container-low p-space-lg xl:col-span-2"
    >
      <div class="flex flex-col">
        <span class="font-label-md text-label-md text-on-surface">Factory reset</span>
        <span class="font-body-sm text-body-sm text-on-surface-variant">
          Restores defaults, clears history, and issues a new device ID.
        </span>
      </div>
      <!--
        Two-step confirmation, in place rather than in a dialog. A factory reset
        clears history and issues a new device id — every other peer sees this
        device as a stranger afterwards — and it is not undoable, so a single
        misplaced click must not be enough. The destructive action is also the
        one that changes wording ("Reset everything"), so the second press is a
        deliberate read rather than a reflex.
      -->
      <button
        v-if="!confirmingReset"
        type="button"
        class="flex items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface hover:bg-surface-bright"
        @click="confirmingReset = true"
      >
        <RotateCcw :size="16" />
        <span>Reset</span>
      </button>
      <div v-else class="flex items-center gap-space-sm">
        <button
          type="button"
          class="rounded-md px-space-md py-space-sm font-label-md text-label-md text-on-surface-variant hover:text-on-surface"
          @click="confirmingReset = false"
        >
          Cancel
        </button>
        <button
          type="button"
          class="rounded-md bg-error-container px-space-md py-space-sm font-label-md text-label-md font-semibold text-on-error-container hover:opacity-90"
          @click="reset"
        >
          Reset everything
        </button>
      </div>
    </section>

    <!--
      Android's folder picker. Fixed to `directory` mode, and the handler takes
      `paths[0]` because the browser emits an array for both modes — in
      directory mode it always holds exactly one entry.
    -->
    <FileBrowser
      :open="browserOpen"
      mode="directory"
      @pick="(paths) => applyDownloadDir(paths[0])"
      @close="browserOpen = false"
    />
  </div>
</template>
