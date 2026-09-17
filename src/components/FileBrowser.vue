<!--
  FileBrowser.vue — the in-app file and folder picker used on Android
  (a documented deviation from FR-2.1 / AND-4; see PRD §15.1).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { ChevronUp, File, Folder, X } from "lucide-vue-next";

import { formatBytes, truncateMiddle } from "@/composables/useFormat";
import { useTransferStore } from "@/stores/useTransferStore";

/**
 * In-app file picker, used on Android instead of the system dialog.
 *
 * Android's SAF picker hands back `content://` URIs. Media-provider ids contain
 * no filename, so photos could only ever be saved as `shared-<timestamp>` —
 * recovering the real name needs an `OpenableColumns.DISPLAY_NAME` query from
 * Java. With all-files access granted, browsing real paths avoids the problem
 * rather than working around it: every pick carries its true name, and folders
 * can be walked.
 */
interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
}

interface DirListing {
  path: string;
  parent: string | null;
  entries: DirEntry[];
}

const props = defineProps<{
  open: boolean;
  /** `files` multi-selects files; `directory` picks the folder being viewed. */
  mode: "files" | "directory";
}>();

const emit = defineEmits<{
  close: [];
  pick: [paths: string[]];
}>();

const transfers = useTransferStore();

const listing = ref<DirListing | null>(null);
const selected = ref(new Set<string>());
const loading = ref(false);

/**
 * What "ready to confirm" means depends on the mode.
 *
 * In `directory` mode the current folder *is* the selection, so merely having
 * a listing is enough. In `files` mode the user must have ticked something.
 */
const canConfirm = computed(() =>
  props.mode === "directory" ? !!listing.value : selected.value.size > 0,
);

/** Read one directory. `undefined` asks Rust for the platform's natural root. */
async function load(path?: string) {
  loading.value = true;
  try {
    // `?? null` because the command's parameter is `Option<String>`: undefined
    // would be omitted from the JSON payload rather than sent as a null.
    listing.value = await invoke<DirListing>("list_directory", { path: path ?? null });
  } catch (error) {
    // Unreadable directories are routine here, not exceptional — Android
    // denies several system paths outright even with all-files access. The
    // previous listing is left in place so the browser stays usable.
    const payload = error as { message?: string; code?: string };
    transfers.toast({
      kind: "error",
      title: payload.message ?? "Couldn't open that folder.",
      code: payload.code,
    });
  } finally {
    loading.value = false;
  }
}

/** Reset and reload each time the browser is opened. */
watch(
  () => props.open,
  (open) => {
    // Opening edge only; closing needs no work.
    if (!open) return;
    // Cleared so a previous session's ticks do not carry into this one — the
    // user who cancelled and reopened expects a clean slate.
    selected.value = new Set();
    void load();
  },
  { immediate: true },
);

/**
 * Handle a tap on an entry: navigate into folders, toggle files.
 *
 * One handler for both because a directory listing mixes them, and the entry's
 * own `isDir` is the only thing that distinguishes the intent.
 */
function activate(entry: DirEntry) {
  // Directories always navigate, in both modes. In `directory` mode this is the
  // only interaction: you select a folder by standing in it and confirming.
  if (entry.isDir) {
    void load(entry.path);
    return;
  }

  // A file tapped in `directory` mode does nothing — silently, because
  // disabling or greying every file would clutter a view whose whole purpose is
  // navigating past them.
  if (props.mode !== "files") return;

  // Toggle membership. Reassigned rather than mutated so the computed tracking
  // it re-evaluates — Vue's reactivity does not observe `Set.add`/`delete` on a
  // plain `ref`, so mutating in place would leave the count and the Confirm
  // button stale.
  const next = new Set(selected.value);
  if (next.has(entry.path)) {
    next.delete(entry.path);
  } else {
    next.add(entry.path);
  }
  selected.value = next;
}

function confirm() {
  // Mirrors `canConfirm`'s two modes. Each guard is redundant with the
  // disabled button but keeps the function safe to call directly.
  if (props.mode === "directory") {
    // Emits a single-element array so the consumer's handler signature is the
    // same for both modes.
    if (listing.value) emit("pick", [listing.value.path]);
    return;
  }
  if (selected.value.size > 0) emit("pick", [...selected.value]);
}
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-150"
    leave-active-class="transition-opacity duration-150"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <div
      v-if="open"
      class="fixed inset-0 z-60 flex items-end justify-center bg-[rgba(9,13,22,0.75)] backdrop-blur-[8px] sm:items-center"
      role="dialog"
      aria-modal="true"
      aria-label="Choose files"
      @click.self="emit('close')"
    >
      <div
        class="flex max-h-[85vh] w-full max-w-xl flex-col gap-space-sm rounded-t-xl border border-white/15 bg-surface-container p-space-lg sheet-safe sm:rounded-xl"
      >
        <header class="flex items-start justify-between gap-space-md">
          <div class="flex min-w-0 flex-col">
            <!-- Mode drives the wording in four places: this heading, the
                 footer hint, the confirm label, and `canConfirm`. -->
            <h2 class="font-headline-lg text-headline-lg text-on-surface">
              {{ mode === "directory" ? "Choose folder" : "Choose files" }}
            </h2>
            <!-- Current path, middle-truncated to fit; the full value stays
                 available as a tooltip. An ellipsis stands in until the first
                 listing arrives. -->
            <span class="telemetry truncate text-on-surface-variant" :title="listing?.path">
              {{ listing ? truncateMiddle(listing.path, 44) : "…" }}
            </span>
          </div>
          <button
            type="button"
            class="rounded-md p-1.5 text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface"
            aria-label="Close"
            @click="emit('close')"
          >
            <X :size="18" />
          </button>
        </header>

        <!--
          "Up one level", hidden at the root where `parent` is null. Rust
          decides what counts as the root, so the browser cannot be navigated
          above the storage volume it started in.
        -->
        <button
          v-if="listing?.parent"
          type="button"
          class="flex items-center gap-space-sm rounded-md bg-surface-container-lowest p-space-sm text-left hover:bg-surface-container-high"
          @click="load(listing.parent ?? undefined)"
        >
          <ChevronUp :size="18" class="text-on-surface-variant" />
          <span class="font-label-md text-label-md text-on-surface-variant">Up one level</span>
        </button>

        <div class="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto">
          <!--
            Loading is tested before emptiness, and the order matters: during a
            load the previous listing is still in `listing`, so without this
            first branch a slow read of a large directory would briefly show
            stale entries, and a read starting from null would flash "This
            folder is empty" before the real contents arrived.
          -->
          <p
            v-if="loading"
            class="px-space-sm py-space-md font-body-md text-body-md text-on-surface-variant"
          >
            Loading…
          </p>
          <p
            v-else-if="!listing?.entries.length"
            class="px-space-sm py-space-md font-body-md text-body-md text-on-surface-variant"
          >
            This folder is empty.
          </p>

          <!--
            Repetition over the entries. Not chained to the states above with a
            `v-else`: the placeholder and the list are mutually exclusive in
            practice (an empty array yields no rows anyway), and the `?? []`
            fallback covers the pre-first-load null without another branch.

            Keyed by full path, which is unique within a directory, so rows keep
            their identity — and their selection ring — across a re-read.
          -->
          <button
            v-for="entry in listing?.entries ?? []"
            :key="entry.path"
            type="button"
            class="flex items-center gap-space-sm rounded-md p-space-sm text-left transition-colors"
            :class="
              selected.has(entry.path)
                ? 'bg-primary-container/25 ring-1 ring-primary/50'
                : 'hover:bg-surface-container-high'
            "
            @click="activate(entry)"
          >
            <!-- Folder vs. file, in both the glyph and its colour: folders are
                 accented because they are the navigable ones. -->
            <component
              :is="entry.isDir ? Folder : File"
              :size="18"
              class="shrink-0"
              :class="entry.isDir ? 'text-primary' : 'text-on-surface-variant'"
            />
            <span class="min-w-0 flex-1 truncate font-body-md text-body-md text-on-surface">
              {{ entry.name }}
            </span>
            <!-- Size only for files. Rust reports 0 for directories rather than
                 recursing, and "0 B" beside a folder would be misleading. -->
            <span v-if="!entry.isDir" class="telemetry shrink-0 text-outline">
              {{ formatBytes(entry.size) }}
            </span>
          </button>
        </div>

        <div
          class="flex items-center justify-between gap-space-md border-t border-outline-variant/40 pt-space-sm"
        >
          <!--
            In `directory` mode the hint explains the non-obvious rule — there
            is no folder to tick, so the one you are standing in is the answer.
            In `files` mode a running count is the more useful readout.
          -->
          <span class="telemetry text-on-surface-variant">
            {{
              mode === "directory"
                ? "Sends the folder you are viewing"
                : `${selected.size} selected`
            }}
          </span>
          <button
            type="button"
            class="rounded-md bg-primary-container px-space-lg py-space-sm font-label-md text-label-md font-semibold text-on-primary-container transition-colors hover:bg-primary hover:text-on-primary disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!canConfirm"
            @click="confirm"
          >
            {{ mode === "directory" ? "Use this folder" : "Add selected" }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>
