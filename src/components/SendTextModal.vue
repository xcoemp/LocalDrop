<!--
  SendTextModal.vue — the snippet composer (FR-3.1 through FR-3.4).

  Project: LocalDrop — zero-configuration LAN file and text transfer
  Author:  Emmanuel Paul <pauldukz@gmail.com>
-->
<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { ClipboardPaste, Send, X } from "lucide-vue-next";

import { formatBytes } from "@/composables/useFormat";
import { usePeerStore } from "@/stores/usePeerStore";
import { useTransferStore } from "@/stores/useTransferStore";

const props = defineProps<{ open: boolean; deviceId: string | null }>();
const emit = defineEmits<{ close: [] }>();

const peers = usePeerStore();
const transfers = useTransferStore();

const text = ref("");
const sending = ref(false);
const textarea = ref<HTMLTextAreaElement | null>(null);

/** FR-3.1 — the cap is on bytes, not characters; emoji are 4 bytes each. */
const MAX_TEXT_BYTES = 1024 * 1024;

// Encoded, not `text.length`: FR-3.1's limit is on the wire size, and a string
// of emoji is four times longer in bytes than in JavaScript characters.
const byteLength = computed(() => new TextEncoder().encode(text.value).length);
const oversize = computed(() => byteLength.value > MAX_TEXT_BYTES);

// Resolved live from the store rather than captured when the modal opened, so a
// peer that drops off mid-compose (12 s TTL) disables Send instead of letting
// the user write into a void.
const target = computed(() => (props.deviceId ? (peers.peers.get(props.deviceId) ?? null) : null));

/**
 * Three conditions gate Send: a live destination, some actual content, and no
 * send already in flight. Whitespace alone is not a snippet worth sending, and
 * the `sending` guard prevents a double-click queueing two copies.
 *
 * Note that `oversize` is deliberately *not* here — over 1 MB is allowed and
 * handled by converting to a file (FR-3.4), not blocked.
 */
const canSend = computed(() => !!target.value && text.value.trim().length > 0 && !sending.value);

/**
 * Mirrors the textarea manually instead of using `v-model`.
 *
 * `v-model` ignores `input` events while an IME composition is active. Android's
 * predictive keyboard keeps the whole word in composition until a space or
 * newline commits it, so the bound value stayed empty and Send stayed disabled
 * until the user typed a space. Reading the element directly — and listening to
 * `compositionupdate` as well — keeps the state accurate from the first
 * keystroke.
 */
function syncText(event: Event) {
  text.value = (event.target as HTMLTextAreaElement).value;
}

/** Focus the composer when the modal opens, so the user can type immediately. */
watch(
  () => props.open,
  async (open) => {
    // Only on the opening edge. Focusing on close would pull focus to a hidden
    // element and, on Android, re-raise the soft keyboard over the radar.
    if (!open) return;
    // `nextTick` because the textarea does not exist until Vue has flushed the
    // `v-if` that mounts it — focusing before that is a no-op on null.
    await nextTick();
    textarea.value?.focus();
  },
);

/**
 * FR-3.2 — clipboard to composer in one action.
 *
 * **Appends** rather than replaces. Replacing meant the button could only ever
 * be used once per snippet: copy a link, paste, copy a second link, paste, and
 * the first one was silently gone. Collecting several clippings into one
 * transfer is the common case — a few links, a command and its output, two
 * codes — and appending makes that possible without retyping anything.
 *
 * It is also the safer direction. Appending never destroys what is already in
 * the box, so a mis-click costs one selection to delete rather than everything
 * the user had assembled.
 */
async function paste() {
  try {
    const clip = await readText();

    // Guarded: an empty clipboard returns "" or null, and appending that would
    // add a separator and nothing else.
    if (clip) {
      const current = text.value;

      // A newline between clippings, so a list of links or codes arrives one
      // per line rather than run together into one unreadable string.
      //
      // Skipped when the box is empty or whitespace-only (nothing to separate
      // from, and a leading blank line looks like a mistake), and when the text
      // already ends in a newline (the user's own line break is enough).
      const needsBreak = current.trim().length > 0 && !current.endsWith("\n");

      text.value = current + (needsBreak ? "\n" : "") + clip;
    }
  } catch {
    // The clipboard can be denied or held by another process; say so rather
    // than leaving the button looking broken.
    transfers.toast({ kind: "error", title: "Couldn't read the clipboard." });
  }

  // Focus is restored on both paths, so the user can keep editing either way.
  await nextTick();
  textarea.value?.focus();

  // Caret to the end, and scrolled into view. Without this the cursor stays
  // where it was before the paste — so continued typing would be inserted in
  // the middle of the text that was just added, and a long accumulation would
  // grow off-screen with no indication anything had happened.
  const el = textarea.value;
  if (el) {
    el.setSelectionRange(el.value.length, el.value.length);
    el.scrollTop = el.scrollHeight;
  }
}

async function send() {
  // Re-checked at the call site even though the button is disabled: `send` is
  // also reachable by keyboard, and `!target.value` narrows the type for the
  // `deviceId` read below.
  if (!canSend.value || !target.value) return;

  sending.value = true;
  try {
    await transfers.sendText(target.value.deviceId, text.value);
    // Cleared only on success, so a failed send leaves the text recoverable
    // rather than making the user retype it.
    text.value = "";
    emit("close");
  } catch (error) {
    // Same typed-payload handling as useDragDrop: prefer Rust's message, fall
    // back to a generic one if something else threw (FR-5.6).
    const payload = error as { message?: string; code?: string };
    transfers.toast({
      kind: "error",
      title: payload.message ?? "Couldn't send the snippet.",
      code: payload.code,
    });
  } finally {
    // `finally` so a failure re-enables Send for a retry.
    sending.value = false;
  }
}
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-150"
    leave-active-class="transition-opacity duration-150"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <!-- Layer 3 from DESIGN.md: scrim + frosted sheet. -->
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-end justify-center bg-[rgba(9,13,22,0.75)] backdrop-blur-[8px] sm:items-center"
      role="dialog"
      aria-modal="true"
      aria-label="Send a text snippet"
      @click.self="emit('close')"
      @keydown.esc="emit('close')"
    >
      <!--
        `max-h-dvh` bounds the sheet to the viewport. `.sheet-safe` grows its
        bottom padding by the keyboard height, and without a ceiling that extra
        height would push the heading and the textarea off the *top* of the
        screen instead — trading one invisible half of the sheet for the other.
      -->
      <div
        class="flex max-h-dvh w-full max-w-xl flex-col gap-space-md rounded-t-xl border border-white/15 bg-surface-container p-space-lg sheet-safe sm:rounded-xl"
      >
        <header class="flex items-start justify-between gap-space-md">
          <div class="flex flex-col">
            <h2 class="font-headline-lg text-headline-lg text-on-surface">Quick text</h2>
            <p class="font-body-sm text-body-sm text-on-surface-variant">
              <!--
                Selection on whether a destination resolved. The `v-else` is
                instructional rather than an error: this is reachable via Ctrl+V
                with a peer that has since timed out, and naming the next step
                is more useful than reporting the absence.
              -->
              <template v-if="target">
                Sending to
                <span class="text-primary">{{ target.alias }}</span>
                · {{ target.ip }}
              </template>
              <template v-else>Select a device on the radar first.</template>
            </p>
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
          `:value` + explicit handlers rather than `v-model` — see `syncText`
          above for the IME reason. Both events are bound: `input` covers
          ordinary typing and paste, `compositionupdate` covers the Android
          predictive keyboard's uncommitted word.

          `min-h-0` is what makes the textarea the part that gives way when the
          keyboard squeezes the sheet. A flex item defaults to `min-height:
          auto`, which refuses to shrink below its content — here seven rows —
          so without it the sheet would overflow and something else would have
          to go off-screen. With it, the header, the Paste button and Send all
          stay put and the typing area gets smaller, which is the right thing to
          sacrifice; `overflow-y-auto` keeps the text scrollable at any height.
        -->
        <textarea
          ref="textarea"
          :value="text"
          rows="7"
          @input="syncText"
          @compositionupdate="syncText"
          data-selectable
          placeholder="Paste a URL, a token, a snippet of code… or several"
          class="min-h-0 w-full resize-none overflow-y-auto rounded-md border border-outline-variant/40 bg-surface-container-lowest p-space-md font-body-md text-body-md text-on-surface placeholder:text-outline focus:border-secondary-container focus:outline-none"
        />

        <div class="flex items-center justify-between gap-space-md">
          <div class="flex items-center gap-space-sm">
            <!--
              The tooltip states that this appends. "Paste" conventionally
              replaces a selection, so the behaviour is worth spelling out —
              but the label stays familiar rather than becoming "Append
              clipboard", which reads like a developer wrote it.
            -->
            <button
              type="button"
              class="flex items-center gap-space-xs rounded-md bg-surface-container-high px-space-md py-space-sm font-label-md text-label-md text-on-surface transition-colors hover:bg-surface-bright"
              title="Adds the clipboard to the end — paste several times to send them together"
              @click="paste"
            >
              <ClipboardPaste :size="16" />
              <span>Paste clipboard</span>
            </button>
            <span class="telemetry text-outline">{{ formatBytes(byteLength) }}</span>
          </div>

          <button
            type="button"
            class="flex items-center gap-space-xs rounded-md bg-primary-container px-space-lg py-space-sm font-label-md text-label-md font-semibold text-on-primary-container transition-colors hover:bg-primary hover:text-on-primary disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!canSend"
            @click="send"
          >
            <Send :size="16" />
            <!-- Label reflects the in-flight state, so the disabled button
                 during a send has a visible explanation. -->
            <span>{{ sending ? "Sending…" : "Send snippet" }}</span>
          </button>
        </div>

        <!--
          FR-3.4 — the substitution is stated up front, not after the fact.
          Shown while composing, as soon as `byteLength` crosses the limit, so
          the behaviour is never a surprise on arrival. The store raises a
          matching toast after the send, from Rust's authoritative answer.
        -->
        <p v-if="oversize" class="font-body-sm text-body-sm text-on-surface-variant">
          This snippet is over 1 MB, so it will be delivered as a
          <span class="text-tertiary">.txt file</span> instead.
        </p>
      </div>
    </div>
  </Transition>
</template>
