<!--
  ToastStack.vue — transient notifications, bottom-right (FR-5.6, FR-3.3).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { CircleAlert, CircleCheck, Info, X } from "lucide-vue-next";

import { useTransferStore } from "@/stores/useTransferStore";

/** FR-5.6 — transient feedback; the durable record lives in history. */
const transfers = useTransferStore();

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

    No `w-full`: `.toast-stack` anchors both left and right, so the width comes
    from the space between them. `w-full` would force 100% of the viewport on
    top of those offsets and push the stack off the left edge — see the note in
    style.css. `max-w-sm` then caps it on a desktop window, and `ml-auto` sends
    the slack to the left so it stays bottom-right there.
  -->
  <div
    class="toast-stack pointer-events-none fixed z-70 ml-auto flex max-w-sm flex-col gap-space-sm"
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
            No copy-details action here, deliberately.

            A toast lasts a few seconds and can be one of a stack of three; an
            action inside it is something the user has to catch before it
            disappears, which makes it a poor home for anything worth keeping.
            `toast.code` and `toast.detail` are still carried on the payload and
            are still reachable — HistoryRow exposes them on the matching entry,
            which persists and can be read at leisure. So the toast stays a
            plain statement of what happened.
          -->
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
