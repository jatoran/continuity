import { applySelectionToInput } from "./coordinates.js";
import { renderProjectionViewport, synchronizeProjectionScroll } from "./projection.js";
import { revealProjectedRange } from "./projection_reveal.js";
import { renderSelectionOverlays } from "./selection_overlays.js";
import { synchronizeScrollExtent } from "./scroll_extent.js";
import {
  activeScroller,
  synchronizeShieldExtent,
} from "./scroll_surface.js";

/** Component-agent-owned textarea viewport and synchronization state. */
export class EditorInputSynchronizer {
  #isAtBottom = true;
  #pendingReveal = null;
  #scrollLeft = 0;
  #scrollTop = 0;
  #surface;

  /** Bind the layer elements once; every method reads them from here. */
  configure(surface) {
    this.#surface = surface;
  }

  get scrollTop() {
    return this.#scrollTop;
  }

  set scrollTop(value) {
    this.#scrollTop = value;
  }

  /** Queue host navigation until the projection has rendered its new selection. */
  requestReveal(range, align, shouldRealize = true) {
    this.#pendingReveal = { range, align, shouldRealize };
  }

  /** Element whose scroll offset currently drives the projection. */
  get scroller() {
    return activeScroller(this.#surface);
  }

  onScroll(snapshot) {
    const { input, projection, affordances, carets } = this.#surface;
    // A hidden input (display:none tab switch) has no layout and reads scroll
    // zero; adopting that would clobber the viewport being kept for re-show.
    if (input.clientWidth === 0 && input.clientHeight === 0) return;
    const scroller = this.scroller;
    if (scroller === input) {
      // Scrolling realizes projected detail, which can change how many visual
      // rows the projection occupies; re-cover the extent before adopting it.
      synchronizeScrollExtent(input, projection);
    } else {
      synchronizeShieldExtent(this.#surface);
      // Keep the textarea's own offset in step so the measurement paths that
      // still read it (resize anchoring, caret mirrors) stay coherent.
      input.scrollTop = scroller.scrollTop;
    }
    this.#isAtBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
    this.#scrollLeft = scroller.scrollLeft;
    this.#scrollTop = scroller.scrollTop;
    synchronizeProjectionScroll(scroller, projection, affordances);
    renderSelectionOverlays(carets, projection, snapshot);
  }

  apply(snapshot, change, shouldRevealCaret) {
    const { input } = this.#surface;
    const scroller = this.scroller;
    const isShieldScrolling = scroller !== input;
    const previousTextLength = computePreviousTextLength(snapshot.text.length, change?.edits);
    const wasAtDocumentEnd = input.selectionEnd === previousTextLength;
    this.#scrollLeft = scroller.scrollLeft;
    this.#scrollTop = scroller.scrollTop;
    const selection = snapshot.selections[0];
    if (shouldRevealCaret && selection) {
      this.requestReveal(
        { start: selection.head, end: selection.head },
        "nearest",
        false,
      );
    }
    synchronizeTextarea(
      input,
      snapshot,
      change,
      isShieldScrolling ? input.scrollTop : this.#scrollTop,
      isShieldScrolling ? input.scrollLeft : this.#scrollLeft,
    );
    if (shouldRevealCaret && wasAtDocumentEnd && this.#isAtBottom) {
      scroller.scrollTop = 1_000_000_000;
      if (isShieldScrolling) input.scrollTop = scroller.scrollTop;
    }
    return inputSelectionKey(input);
  }

  afterRender(snapshot) {
    const { input, projection, affordances, carets } = this.#surface;
    // Renders that land while the host has the editor hidden see zero layout;
    // skip so the tracked viewport survives until re-show.
    if (input.clientWidth === 0 && input.clientHeight === 0) return;
    const scroller = this.scroller;
    if (scroller === input) {
      synchronizeScrollExtent(input, projection);
    } else {
      synchronizeShieldExtent(this.#surface);
      input.scrollTop = scroller.scrollTop;
    }
    synchronizeProjectionScroll(scroller, projection, affordances);
    let hasFreshOverlays = false;
    let measuredCaret = null;
    if (this.#pendingReveal && !this.#pendingReveal.shouldRealize) {
      measuredCaret = renderSelectionOverlays(carets, projection, snapshot).primaryCaret;
      hasFreshOverlays = true;
    }
    const revealResult = this.#pendingReveal
      ? revealProjectedRange(
        projection, scroller, this.#pendingReveal.range, this.#pendingReveal.align,
        measuredCaret ? [measuredCaret] : [],
      )
      : null;
    if (revealResult) {
      const shouldRealize = this.#pendingReveal.shouldRealize
        || Math.abs(revealResult.scrollDelta) > scroller.clientHeight;
      if (revealResult.shouldCorrect) {
        hasFreshOverlays = false;
        synchronizeProjectionScroll(scroller, projection, affordances);
        if (shouldRealize) {
          renderProjectionViewport(projection, scroller);
          if (scroller === input) synchronizeScrollExtent(input, projection);
          else synchronizeShieldExtent(this.#surface);
          synchronizeProjectionScroll(scroller, projection, affordances);
          revealProjectedRange(
            projection, scroller, this.#pendingReveal.range, this.#pendingReveal.align,
          );
        }
      }
      this.#pendingReveal = null;
      if (scroller !== input) input.scrollTop = scroller.scrollTop;
    }
    this.#scrollLeft = scroller.scrollLeft;
    this.#scrollTop = scroller.scrollTop;
    this.#isAtBottom = this.#scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
    synchronizeProjectionScroll(scroller, projection, affordances);
    if (!hasFreshOverlays) renderSelectionOverlays(carets, projection, snapshot);
    // The clipboard bar anchors to the painted highlight, so it can only be
    // placed once the overlay exists — anywhere earlier finds nothing to
    // measure and hides itself.
    this.#surface.onSelectionPainted?.();
  }
}

/** Incrementally synchronize the semantic textarea with an engine snapshot. */
export function synchronizeTextarea(
  input, snapshot, change, scrollTop, scrollLeft,
) {
  if (change?.edits) {
    change.edits.forEach((edit) => input.setRangeText(
      edit.insertedText, edit.startUtf16, edit.endUtf16, "preserve",
    ));
  } else if (input.value !== snapshot.text) {
    input.value = snapshot.text;
  }
  const selection = snapshot.selections[0];
  if (selection) {
    applySelectionToInput(snapshot.text, selection, input);
  }
  input.scrollLeft = scrollLeft;
  input.scrollTop = scrollTop;
}

/** Cheap identity for detecting a browser selection that actually moved. */
export function inputSelectionKey(input) {
  return `${input.selectionStart}:${input.selectionEnd}:${input.selectionDirection}`;
}

function computePreviousTextLength(currentLength, edits = []) {
  return edits.reduce((length, edit) => (
    length - edit.insertedText.length + edit.endUtf16 - edit.startUtf16
  ), currentLength);
}
