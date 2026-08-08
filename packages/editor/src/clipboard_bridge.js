// Clipboard access for editor-initiated actions.
//
// Split from the element because the browser will refuse these for reasons the
// editor cannot fix — an insecure context has no `navigator.clipboard` at all,
// and a read can be denied by permission policy. Every path therefore has a
// host fallback: the editor asks, and a host that has its own clipboard route
// can answer. Clipboard *events* (copy/cut/paste on the textarea) are a separate
// concern and stay in `input_events.js`.

import { writeClipboardText } from "./code_affordances.js";

/**
 * Text this editor last copied or cut.
 *
 * A permissions policy can deny `clipboard-read` outright — an embedding frame
 * that did not delegate it is the usual cause — and a document cannot grant
 * itself what its parent withheld. Nothing inside the page lifts that, so paste
 * would simply be dead there. Remembering what the editor itself put on the
 * clipboard makes copy/cut/paste within the editor work regardless of policy;
 * only pasting from another application still needs the browser's permission.
 */
let lastCopiedText = null;

/** Bind the clipboard fallbacks to one editor's textarea and event surface. */
export function createClipboardHooks({ input, emitRequest, emitError }) {
  return {
    emitRequest,
    emitError,
    refocus: () => input.focus({ preventScroll: true }),
    /**
     * Legacy paste, straight into the textarea. Chrome refuses it, but Electron
     * and several WebView hosts still run it — and when it works the browser
     * never had to hand the page the clipboard contents at all.
     */
    pasteViaCommand: () => {
      const before = input.value;
      const start = input.selectionStart;
      input.focus({ preventScroll: true });
      try {
        if (!document.execCommand("paste")) return null;
      } catch { return null; }
      if (input.value === before) return null;
      // Recover what landed, then undo it: the editor applies the insert itself
      // so the engine stays the single writer of document state.
      const inserted = input.value.slice(start, input.selectionEnd ?? start);
      input.value = before;
      input.setSelectionRange(start, start);
      return inserted;
    },
    copySelection: () => {
      input.focus({ preventScroll: true });
      return input.selectionEnd > input.selectionStart && document.execCommand("copy");
    },
  };
}

/**
 * Read clipboard text.
 *
 * `clipboard.readText()` is the only programmatic read the web platform offers,
 * and a permissions policy can refuse it outright — an embedding frame that did
 * not delegate `clipboard-read` is the usual cause, and nothing inside the
 * document can lift that. Two escapes remain before giving up: the legacy paste
 * command, which several embedders (Electron, some WebViews) still honour, and
 * the host, which may have a clipboard route the document does not.
 */
export async function readClipboardText(hooks) {
  if (navigator.clipboard?.readText) {
    try {
      const text = await navigator.clipboard.readText();
      if (typeof text === "string") return text;
    } catch (error) {
      hooks.emitError(error);
    }
  }
  const pasted = hooks.pasteViaCommand?.();
  if (typeof pasted === "string" && pasted.length > 0) return pasted;
  if (typeof lastCopiedText === "string" && lastCopiedText.length > 0) return lastCopiedText;
  hooks.emitRequest("pasteText", {});
  return null;
}

/**
 * Write text, delegating to the host when the browser refuses.
 *
 * The editor's own textarea already holds the selection, so copying it in place
 * is tried before the detached-textarea fallback: that fallback has to steal
 * focus, and on a phone stealing focus mid-gesture is what makes the copy fail
 * silently. `execCommand` also needs the user gesture that is still on the
 * stack, so nothing here may await before reaching it.
 */
export async function copyText(text, hooks) {
  // Remembered before any transport is attempted: even a copy the browser
  // refuses to put on the system clipboard stays pasteable inside the editor.
  lastCopiedText = text;
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch { /* insecure context or denied policy; fall through */ }
  }
  try {
    if (hooks.copySelection()) return;
    await writeClipboardText(text);
  } catch (error) {
    hooks.emitRequest("copyText", { text });
    hooks.emitError(error);
  }
}

/**
 * Copy a fenced code block. Unlike the selection actions this rethrows, because
 * the affordance that triggered it reports success or failure in its own label.
 */
export async function copyCodeBlock(text, hooks) {
  try {
    await writeClipboardText(text);
  } catch (error) {
    hooks.emitRequest("copyText", { text });
    hooks.emitError(error);
    throw error;
  } finally {
    hooks.refocus();
  }
}
