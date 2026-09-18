<!--
  ToastStack.vue — transient notifications, bottom-right (FR-5.6, FR-3.3).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { ref } from "vue";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CircleAlert, CircleCheck, Info, X } from "lucide-vue-next";

import { useTransferStore, type Toast } from "@/stores/useTransferStore";

/** FR-5.6 — transient feedback; the durable record lives in history. */
const transfers = useTransferStore();

/**
 * Which toast last had its details copied, so the button can confirm.
 *
 * Tracked by id rather than a boolean because several error toasts can be
 * stacked at once, and a shared flag would make every one of them claim to
 * have been copied.
 */
const copiedId = ref<number | null>(null);

/**
 * Put the technical cause on the clipboard: the code, the message the user
 * saw, and the raw detail if there was one.
 *
 * Assembled into one block so a bug report carries all three together —
 * pasting only the code loses what the user was told, and pasting only the
 * message loses the part that identifies the failure in the source.
 */
async function copyDetails(toast: Toast) {
  const lines = [toast.code, toast.title, toast.detail].filter(Boolean);

  try {
    await writeText(lines.join("\n"));
    copiedId.value = toast.id;
    // Reverted so the button does not read "Copied" for the rest of the
    // toast's life, which would be wrong if it were clicked again.
    setTimeout(() => {
      if (copiedId.value === toast.id) copiedId.value = null;
    }, 2000);
  } catch {
    // The clipboard can be locked by another process on Windows. Nothing to
    // report — the label simply does not change, and the title attribute
    // still shows the code for manual transcription.
  }
}

/**
 * Lookup tables rather than `switch` or a `v-if` chain.
 *
 * The toast kind is a closed three-variant union, so indexing a const object is
 * both exhaustive at compile time and a single expression in the template.
 * Two separate maps, not one map of pairs, because the icon is bound to
 * `:is` and the accent to `:class` — they are consumed in different places.
 */
const icons = {
  info: Info,
  success: CircleCheck,
  error: CircleAlert,
} as const;

const accents = {
  info: "text-tertiary",
  success: "text-primary",
  error: "text-error",
} as const;
</script>

<template>
  <!--
    `pointer-events-none` on the container with `pointer-events-auto` on each
    toast: the stack spans a wide fixed region, and without this the empty space
    beside a toast would swallow clicks meant for the UI underneath.

    That only covers the empty space, though — a toast itself must stay
    clickable to be dismissed, so it still blocks whatever is under it. Hence
    `.toast-stack` (style.css) rather than a plain `bottom-*`: on mobile it
    offsets the whole stack above the bottom navigation and the system gesture
    inset, so the two never occupy the same pixels.

    aria-live="polite" announces new toasts to a screen reader without
    interrupting whatever is being read (UI-7).
  -->
  <div
    class="toast-stack pointer-events-none fixed z-70 flex w-full max-w-sm flex-col gap-space-sm"
    role="status"
    aria-live="polite"
  >
    <TransitionGroup
      enter-active-class="transition duration-200"
      leave-active-class="transition duration-200"
      enter-from-class="opacity-0 translate-y-2"
      leave-to-class="opacity-0 translate-y-2"
    >
      <!--
        Repetition over the live toast list. Keyed on the store's monotonic id
        rather than the array index — with an index key, dismissing the middle
        toast would make Vue reuse the wrong DOM nodes and the remaining toasts
        would appear to change content mid-animation.
      -->
      <div
        v-for="toast in transfers.toasts"
        :key="toast.id"
        class="pointer-events-auto flex items-start gap-space-sm rounded-md border border-white/10 bg-surface-container p-space-md shadow-lg"
      >
        <component
          :is="icons[toast.kind]"
          :size="18"
          class="mt-0.5 shrink-0"
          :class="accents[toast.kind]"
        />
        <div class="flex min-w-0 flex-1 flex-col">
          <span class="font-label-md text-label-md text-on-surface">{{ toast.title }}</span>

          <!-- Optional detail line; omitted entirely rather than left blank. -->
          <span
            v-if="toast.body"
            data-selectable
            class="truncate font-body-sm text-body-sm text-on-surface-variant"
          >
            {{ toast.body }}
          </span>

          <!--
            FR-5.6's copyable error code, as an action rather than visible text.
            Showing `ERR_WRITE_FAILED` on screen told the user nothing they
            could act on and read as leaked debug output, but support still
            needs it — so it moves behind one click.
          -->
          <button
            v-if="toast.code"
            type="button"
            class="mt-0.5 self-start rounded-sm font-label-sm text-label-sm text-outline underline decoration-dotted underline-offset-2 transition-colors hover:text-on-surface-variant"
            :title="`Copy technical details (${toast.code})`"
            @click="copyDetails(toast)"
          >
            {{ copiedId === toast.id ? "Copied" : "Copy details" }}
          </button>
        </div>

        <!-- Manual dismissal, in addition to the store's TTL timer. -->
        <button
          type="button"
          class="rounded-sm p-1 text-on-surface-variant hover:text-on-surface"
          aria-label="Dismiss"
          @click="transfers.dismissToast(toast.id)"
        >
          <X :size="14" />
        </button>
      </div>
    </TransitionGroup>
  </div>
</template>
