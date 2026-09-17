<!--
  SendView.vue — the Send Payload screen: pick a destination, queue files or a
  folder, review, send (FR-2.1, §10.2).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { platform } from "@tauri-apps/plugin-os";
import { FileUp, FolderUp, Radar, Send, Type } from "lucide-vue-next";

import EmptyState from "@/components/EmptyState.vue";
import FileBrowser from "@/components/FileBrowser.vue";
import OsIcon from "@/components/OsIcon.vue";
import { fileName, truncateMiddle } from "@/composables/useFormat";
import { usePeerStore } from "@/stores/usePeerStore";
import { useTransferStore } from "@/stores/useTransferStore";

/**
 * The send workspace (send_drag_and_drop_workspace mockup).
 *
 * Files are queued locally first so the user can review the batch before it
 * leaves the machine — a send is not undoable.
 */
const emit = defineEmits<{ sendText: [deviceId: string] }>();

const peers = usePeerStore();
const transfers = useTransferStore();

const queue = ref<string[]>([]);
const sending = ref(false);

/** iOS has no equivalent of a browsable filesystem, so folders stay hidden. */
const canSendFolders = platform() !== "ios";

const target = computed(() => peers.selected);
/**
 * Three conditions, matching the snippet composer: a destination that is still
 * on the radar, something queued, and no dispatch already running.
 */
const canSend = computed(() => !!target.value && queue.value.length > 0 && !sending.value);

const browserOpen = ref(false);
const browserMode = ref<"files" | "directory">("files");

/** Android uses the in-app browser for every pick; desktop uses native dialogs. */
const isAndroid = platform() === "android";

/**
 * A `content://` URI that carries no filename.
 *
 * No longer produced by the picker, but a URI can still reach the queue from
 * elsewhere (a share intent, say), so the queue still labels it sensibly rather
 * than showing a raw provider id.
 */
function isUnnamedContentUri(path: string): boolean {
  // A real filesystem path is the overwhelmingly common case, so reject early.
  if (!path.startsWith("content://")) return false;

  // Decoded because SAF percent-encodes the segment (`image%3A1000000034`).
  const tail = decodeURIComponent(path.split("/").pop() ?? "");

  // A dot is the heuristic for "carries a filename". Crude, but the two shapes
  // are far apart in practice: a document-provider URI ends in something like
  // `photo.jpg`, while a media-provider id ends in `image:1000000034` — no
  // extension, no name, nothing to show the user.
  return !tail.includes(".");
}

/**
 * Android deliberately does not use the system file chooser.
 *
 * SAF returns `content://` URIs, and a pick from Photos or Recent is an opaque
 * database id (`document/image%3A1000000034`) with no filename in it at all.
 * Every such file had to be invented a `shared-<timestamp>` name on arrival.
 * The in-app browser works on real paths, so names and folders both survive.
 *
 * Desktop keeps its native dialog: it returns real paths already.
 */
async function browseFiles() {
  // Platform fork: the in-app browser on Android, the OS dialog on desktop.
  if (isAndroid) {
    browseDevice("files");
    return;
  }

  const picked = await open({ multiple: true, directory: false, title: "Select files to send" });

  // Null means the user cancelled the dialog — not an error, and not a reason
  // to disturb an already-queued batch.
  if (!picked) return;

  // `multiple: true` usually yields an array, but the plugin returns a bare
  // string for a single selection on some platforms, so both are normalised.
  // Appended rather than replaced, so browsing twice accumulates.
  queue.value = [...queue.value, ...(Array.isArray(picked) ? picked : [picked])];
}

/**
 * Folder picking uses the in-app browser on Android.
 *
 * Android's folder picker returns a SAF *tree* URI. Even with all-files access
 * that route stays unreliable — the grant is per-tree and the URI only maps to
 * a path for the external-storage provider — and when it fails it fails
 * silently, walking nothing. Browsing real paths is deterministic.
 */
async function browseFolder() {
  if (isAndroid) {
    browseDevice("directory");
    return;
  }
  const picked = await open({ multiple: false, directory: true, title: "Select a folder to send" });

  // Cancelled, or — defensively — an array from a plugin configured for single
  // selection. An array here would queue a nested value the backend cannot read.
  if (!picked || Array.isArray(picked)) return;
  queue.value = [...queue.value, picked];
}

function browseDevice(mode: "files" | "directory") {
  // One browser component serves both pickers; the mode decides its behaviour.
  browserMode.value = mode;
  browserOpen.value = true;
}

function onBrowserPick(paths: string[]) {
  queue.value = [...queue.value, ...paths];
  browserOpen.value = false;
}

/**
 * Queue label: opaque provider ids are unreadable, so describe them instead.
 *
 * Reached only for a URI arriving from outside the app (a share intent), since
 * the in-app browser always yields real paths.
 */
function queueLabel(path: string): string {
  // The normal path: just the filename.
  if (!isUnnamedContentUri(path)) return fileName(path);

  // Otherwise the provider id's prefix (`image:`, `video:`, `audio:`) is the
  // only hint about what the file is, so it becomes a human noun.
  const kind = decodeURIComponent(path.split("/").pop() ?? "").split(":")[0];
  const noun =
    kind === "image" ? "Photo" : kind === "video" ? "Video" : kind === "audio" ? "Audio" : "File";

  // The parenthetical sets expectations: the real name is resolved by Rust's
  // JNI `DISPLAY_NAME` lookup at send time, not here.
  return `${noun} (name assigned on send)`;
}

function removeAt(index: number) {
  // Filtered by index, not by value: the same file can legitimately appear
  // twice (browsed from two locations), and removing by path would drop both.
  queue.value = queue.value.filter((_, i) => i !== index);
}

async function dispatch() {
  // Re-checked for keyboard activation, and to narrow `target` for the read.
  if (!canSend.value || !target.value) return;

  sending.value = true;
  try {
    await transfers.sendFiles(target.value.deviceId, queue.value);
    // Emptied only on success — Rust has taken ownership of the batch by then.
    // On failure the queue survives so the user can retry without re-picking.
    queue.value = [];
  } catch (error) {
    const payload = error as { message?: string; code?: string };
    transfers.toast({
      kind: "error",
      title: payload.message ?? "Couldn't start the transfer.",
      code: payload.code,
    });
  } finally {
    sending.value = false;
  }
}
</script>

<template>
  <div class="flex flex-col gap-space-lg">
    <!-- Target peer -->
    <section class="flex flex-col gap-space-sm">
      <h2
        class="px-space-xs font-label-sm text-label-sm font-semibold uppercase tracking-widest text-outline"
      >
        Active peer target
      </h2>

      <!-- UI-4 — with no peers there is nothing to select, so explain rather
           than render an empty grid. -->
      <EmptyState
        v-if="!peers.list.length"
        :icon="Radar"
        title="No devices yet"
        body="Peers appear automatically once LocalDrop is open on another device on this network."
      />

      <div
        v-else
        class="grid grid-cols-1 gap-space-sm sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4"
      >
        <!--
          A compact selector, not the full PeerCard: this screen is about the
          payload, and repeating the radar's telemetry here would bury it.

          Busy peers are *not* disabled, unlike the drop picker — FR-2.6 queues
          per peer, so choosing a busy device is legitimate: the batch simply
          runs after its current one.
        -->
        <button
          v-for="peer in peers.list"
          :key="peer.deviceId"
          type="button"
          class="flex items-center gap-space-sm rounded-md border p-space-sm text-left transition-colors"
          :class="
            peers.selectedId === peer.deviceId
              ? 'border-primary bg-surface-container aura-ready'
              : 'border-outline-variant/30 bg-surface-container-low hover:bg-surface-container'
          "
          @click="peers.select(peer.deviceId)"
        >
          <div
            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-surface-container-high text-primary"
          >
            <OsIcon :os="peer.os" :size="18" />
          </div>
          <div class="flex min-w-0 flex-col">
            <span class="truncate font-label-md text-label-md text-on-surface">
              {{ peer.alias }}
            </span>
            <span class="telemetry text-outline">{{ peer.ip }}</span>
          </div>
        </button>
      </div>
    </section>

    <!-- Payload queue -->
    <section class="flex flex-col gap-space-sm">
      <div class="flex items-center justify-between px-space-xs">
        <h2
          class="font-label-sm text-label-sm font-semibold uppercase tracking-widest text-outline"
        >
          Queued payloads
        </h2>
        <!-- Hidden when the queue is already empty. -->
        <button
          v-if="queue.length"
          type="button"
          class="font-label-md text-label-md text-on-surface-variant hover:text-on-surface"
          @click="queue = []"
        >
          Clear queue
        </button>
      </div>

      <div class="flex flex-wrap gap-space-sm">
        <button
          type="button"
          class="flex items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface transition-colors hover:bg-surface-bright"
          @click="browseFiles"
        >
          <FileUp :size="16" />
          <span>Browse files</span>
        </button>
        <!-- Omitted entirely where folders cannot be browsed, rather than
             disabled: a permanently dead button invites repeated attempts. -->
        <button
          v-if="canSendFolders"
          type="button"
          class="flex items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface transition-colors hover:bg-surface-bright"
          @click="browseFolder"
        >
          <FolderUp :size="16" />
          <span>Browse folder</span>
        </button>
        <!--
          Needs a destination up front, because the snippet modal sends
          immediately rather than queueing. The `target &&` in the handler is
          belt-and-braces for keyboard activation.
        -->
        <button
          type="button"
          class="flex items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface transition-colors hover:bg-surface-bright disabled:opacity-40"
          :disabled="!target"
          @click="target && emit('sendText', target.deviceId)"
        >
          <Type :size="16" />
          <span>Send text</span>
        </button>
      </div>

      <div
        v-if="queue.length"
        class="flex flex-col gap-1 rounded-lg bg-surface-container-low p-space-sm"
      >
        <!--
          Repetition over the queue. Keyed on path *and* index, because the same
          file may legitimately be queued twice and a duplicate key would make
          Vue drop one of the rows.
        -->
        <div
          v-for="(path, index) in queue"
          :key="`${path}-${index}`"
          class="flex items-center justify-between gap-space-sm rounded-sm px-space-sm py-space-xs hover:bg-surface-container"
        >
          <span class="truncate font-body-md text-body-md text-on-surface">
            {{ queueLabel(path) }}
          </span>
          <div class="flex shrink-0 items-center gap-space-sm">
            <!-- The full path, so two same-named files from different folders
                 are distinguishable. Dropped on narrow screens. -->
            <span class="telemetry hidden text-outline sm:inline">
              {{ truncateMiddle(path, 36) }}
            </span>
            <button
              type="button"
              class="font-label-sm text-label-sm text-on-surface-variant hover:text-error"
              @click="removeAt(index)"
            >
              Remove
            </button>
          </div>
        </div>
      </div>

      <!-- FR-2.1 — drag-and-drop is the fast path on desktop. Android's webview
           emits no drag events, so the hint would describe something that
           cannot be done there. -->
      <div
        v-else
        class="rounded-lg border border-dashed border-outline-variant/50 bg-surface-container-lowest/60 px-space-lg py-space-xl text-center"
      >
        <p class="font-body-md text-body-md text-on-surface-variant">
          {{
            isAndroid
              ? "Nothing queued yet — browse for files or a folder above."
              : "Drop files anywhere in this window, or browse above."
          }}
        </p>
      </div>
    </section>

    <div class="flex items-center justify-between gap-space-md">
      <!-- Confirms what is about to happen, or names the missing step — the
           same pattern as the snippet modal's header. -->
      <span class="telemetry text-on-surface-variant">
        <template v-if="target">
          {{ queue.length }} item{{ queue.length === 1 ? "" : "s" }} → {{ target.alias }}
        </template>
        <template v-else>Select a destination device above.</template>
      </span>
      <button
        type="button"
        class="flex items-center gap-space-xs rounded-md bg-primary-container px-space-lg py-space-sm font-label-md text-label-md font-semibold text-on-primary-container transition-colors hover:bg-primary hover:text-on-primary disabled:cursor-not-allowed disabled:opacity-40"
        :disabled="!canSend"
        @click="dispatch"
      >
        <Send :size="16" />
        <!-- "Starting…", not "Sending…": this command only enqueues the batch
             and returns; the transfer itself is watched on the Transfers
             screen. Overstating it would make the button look stuck. -->
        <span>{{ sending ? "Starting…" : "Send payload" }}</span>
      </button>
    </div>

    <FileBrowser
      :open="browserOpen"
      :mode="browserMode"
      @pick="onBrowserPick"
      @close="browserOpen = false"
    />
  </div>
</template>
