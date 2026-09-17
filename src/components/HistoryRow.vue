<!--
  HistoryRow.vue — one completed, failed, cancelled, or declined transfer
  (FR-5.4, FR-5.5, FR-5.6).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { platform } from "@tauri-apps/plugin-os";
import { ArrowDownToLine, ArrowUpFromLine, Copy, FolderOpen } from "lucide-vue-next";

import { formatBytes, formatRelativeTime, truncateMiddle } from "@/composables/useFormat";
import { useTransferStore } from "@/stores/useTransferStore";
import type { HistoryEntry } from "@/types/protocol";

const props = defineProps<{ entry: HistoryEntry }>();
const transfers = useTransferStore();

/**
 * Deviation from FR-5.5: history offers **Show in folder only, and only on
 * desktop**. There is no "Open file" action on either platform.
 *
 * Handing the file to the OS to open is the one action here that runs something
 * the user has not inspected, which sits awkwardly next to SEC-4. Revealing it
 * in the file manager gets them to the same place and leaves the decision to
 * open with them. Android has no equivalent — `revealItemInDir` does not
 * resolve to anything a phone can act on — so the row shows no path actions
 * there at all.
 */
const canReveal = platform() !== "android";

/** FR-5.5 — the file may have been moved or deleted since it arrived. */
const fileExists = ref(false);

onMounted(async () => {
  // Two guards, both skipping a pointless IPC call: an outgoing transfer or a
  // snippet has no path to check, and on Android nothing consumes the answer
  // because there is no reveal button to enable.
  if (!props.entry.path || !canReveal) return;
  fileExists.value = await invoke<boolean>("path_exists", { path: props.entry.path });
});

/**
 * Outcome → accent colour. An object lookup rather than a `switch`, since
 * `Outcome` is a closed four-variant union and every arm is a bare value.
 * `failed` and `rejected` share the error colour: both are outcomes the user
 * did not choose.
 */
const outcomeClass = {
  completed: "text-primary",
  cancelled: "text-outline",
  failed: "text-error",
  rejected: "text-error",
} as const;

async function reveal() {
  // Re-checked even though the button is hidden without a path: the guard keeps
  // this safe to call from anywhere, and narrows the type for the call below.
  if (!props.entry.path) return;
  try {
    await revealItemInDir(props.entry.path);
  } catch {
    // The file may have been deleted between the existence check on mount and
    // this click, which is exactly the FR-5.5 case the note covers.
    transfers.toast({ kind: "error", title: "Couldn't open that folder." });
  }
}

async function copyText() {
  if (!props.entry.text) return;
  await writeText(props.entry.text);
  // FR-3.3 — a copy is silent otherwise, so confirm it happened.
  transfers.toast({ kind: "success", title: "Copied to clipboard" });
}
</script>

<template>
  <div
    class="flex items-center justify-between gap-space-md rounded-md bg-surface-container-lowest px-space-md py-space-sm"
  >
    <div class="flex min-w-0 items-center gap-space-sm">
      <!--
        Direction picks the arrow, outcome picks its colour. Unlike TransferRow,
        history keeps the directional arrow even for failures: in a mixed log,
        knowing which way a transfer was going matters more than re-stating an
        outcome that the accent colour and the error line already convey.
      -->
      <component
        :is="entry.direction === 'incoming' ? ArrowDownToLine : ArrowUpFromLine"
        :size="16"
        :class="outcomeClass[entry.outcome]"
      />
      <div class="flex min-w-0 flex-col">
        <span class="truncate font-label-md text-label-md text-on-surface">
          {{ truncateMiddle(entry.label, 40) }}
        </span>
        <span class="telemetry text-on-surface-variant">
          {{ entry.direction === "incoming" ? "from" : "to" }} {{ entry.peerAlias }} ·
          {{ formatBytes(entry.totalBytes) }} · {{ formatRelativeTime(entry.finishedAt) }}
        </span>
        <!--
          FR-5.6 — the cause, plus a copyable code. Present only for failures;
          `errorMessage` and `errorCode` are both null for every other outcome.
        -->
        <span v-if="entry.errorMessage" class="font-body-sm text-body-sm text-error">
          {{ entry.errorMessage }}
          <span class="telemetry text-outline">({{ entry.errorCode }})</span>
        </span>
      </div>
    </div>

    <div class="flex shrink-0 items-center gap-space-xs">
      <!--
        Snippet-only action (FR-3.3). `entry.text` is non-null exactly for text
        payloads, so the presence of the data is itself the condition.
      -->
      <button
        v-if="entry.text"
        type="button"
        class="rounded-md p-1.5 text-on-surface-variant transition-colors hover:bg-surface-container-high hover:text-on-surface"
        title="Copy snippet"
        aria-label="Copy snippet"
        @click="copyText"
      >
        <Copy :size="16" />
      </button>

      <!--
        Desktop only: Android has no file manager to reveal into.

        Two conditions to render, then a third to enable. FR-5.5 requires an
        entry whose file is gone be *disabled with an explanation* rather than
        hidden — the row is still a true record of a transfer that happened, and
        silently dropping the button would look like a bug. So the styling,
        the `disabled` attribute, and the tooltip all key off `fileExists`.
      -->
      <button
        v-if="entry.path && canReveal"
        type="button"
        class="rounded-md p-1.5 transition-colors"
        :class="
          fileExists
            ? 'text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface'
            : 'cursor-not-allowed text-outline/40'
        "
        :disabled="!fileExists"
        :title="fileExists ? 'Show in folder' : 'File moved or deleted'"
        aria-label="Show in folder"
        @click="reveal"
      >
        <FolderOpen :size="16" />
      </button>
    </div>
  </div>
</template>
