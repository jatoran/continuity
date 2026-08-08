// Native caret adoption.
//
// The engine used to learn about browser-driven selection changes only from the
// textarea's `select` event. Per spec `select` fires when a *range* is selected,
// so every collapsed caret move that the platform performs on its own — a
// dragged caret handle, GBoard's spacebar cursor slide, an autocorrect
// replacement, iOS's tap-to-reposition — never reached the engine. The drawn
// caret is painted from engine state while typing goes to the textarea, so the
// two silently drift apart: text lands where the user tapped, the blinking
// caret stays where it was.
//
// `selectionchange` is the only event that covers collapsed moves. It fires
// often, so adoption is coalesced onto an animation frame and skipped unless
// the textarea's selection actually differs from what was last adopted.

import { inputSelectionKey } from "./input_sync.js";
import { applySelectionToInput, selectionFromInput } from "./coordinates.js";
import { selectionsWithInputPrimary } from "./multi_selection.js";
import { renderSelectionOverlays } from "./selection_overlays.js";

/**
 * Adopt the textarea's selection into the engine.
 *
 * Composition is not the brief, exceptional state the rest of this file assumes:
 * Android IMEs hold one open continuously across ordinary typing, so refusing to
 * adopt while composing stranded every selection the user made on a phone. The
 * textarea's own `::selection` is transparent, so a stranded selection is not
 * merely stale — it is invisible, while the platform holds a real one. Offsets
 * map exactly whenever the two buffers still hold the same text, which is the
 * case for any selection gesture that has not also inserted composed characters.
 */
export function applyNativeSelection(ctx) {
  const editor = ctx.editor();
  if (!editor) return;
  const snapshot = editor.snapshot();
  const input = ctx.input;
  const isComposing = ctx.isComposing();
  // Once this touch has produced a projection-owned selection, the platform's
  // competing long-press selection must not overwrite it. The platform selects
  // before it raises `contextmenu` and then cancels the pointer to run its own
  // drag, so the only defence left at this layer is to stop adopting what it
  // reports for the remainder of the gesture.
  if (ctx.touchSelection?.hasClaimed) {
    // Put the owned selection back into the textarea too. Leaving the platform's
    // answer there would make the next keystroke or copy act on text tens of
    // characters from what the reader has highlighted.
    const owned = snapshot.selections[0];
    if (owned && snapshot.text.length === input.value.length) {
      applySelectionToInput(snapshot.text, owned, input);
      ctx.setSelectionKey(inputSelectionKey(input));
    }
    return;
  }
  const canAdopt = !isComposing || snapshot.text.length === input.value.length;
  if (canAdopt) {
    editor.setSelections(ctx.getCollapse()
      ? [selectionFromInput(snapshot.text, input)]
      : selectionsWithInputPrimary(snapshot, input));
    ctx.setSelectionKey(inputSelectionKey(input));
  }
  ctx.setCollapse(false);
  if (!isComposing) {
    ctx.schedule();
    // Offer the clipboard bar for every settled selection, not only one that a
    // touch drag produced: a host `setSelections`, select-all, or a keyboard
    // range is just as selectable and previously showed nothing.
    ctx.updateSelectionActions?.();
    return;
  }
  // The composition preview paints the caret and any same-line selection from
  // the live textarea, but clears the overlay for a selection reaching past the
  // previewed line. Draw those from the engine instead of losing them.
  if (canAdopt && hasMultiLineInputSelection(input)) {
    renderSelectionOverlays(ctx.carets, ctx.projection, editor.snapshot());
  } else {
    ctx.renderComposing();
  }
}

/** Whether a selection reaches past the single line a composition previews. */
function hasMultiLineInputSelection(input) {
  return input.value.slice(input.selectionStart ?? 0, input.selectionEnd ?? 0).includes("\n");
}

/** Adopt platform-driven textarea caret moves that raise no `select` event. */
export class NativeSelectionWatcher {
  #frame;
  #isDestroyed = false;

  /**
   * @param {object} context
   * @param {() => boolean} context.canAdopt whether adoption is safe right now
   * @param {() => string} context.currentKey last selection adopted by the editor
   * @param {() => void} context.adopt pull the textarea selection into the engine
   * @param {HTMLTextAreaElement} context.input
   */
  constructor(context) {
    this.context = context;
  }

  /** Handle one `selectionchange` notification for the owned textarea. */
  onSelectionChange = () => {
    if (this.#isDestroyed || this.#frame !== undefined) return;
    const { input, canAdopt, currentKey } = this.context;
    // `selectionchange` is document-scoped; ignore every other field on the page.
    if (input.ownerDocument.activeElement !== input && input.getRootNode().activeElement !== input) {
      return;
    }
    if (!canAdopt() || inputSelectionKey(input) === currentKey()) return;
    this.#frame = requestAnimationFrame(() => {
      this.#frame = undefined;
      if (this.#isDestroyed || !canAdopt()) return;
      if (inputSelectionKey(input) === currentKey()) return;
      this.context.adopt();
    });
  };

  destroy() {
    this.#isDestroyed = true;
    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
  }
}
