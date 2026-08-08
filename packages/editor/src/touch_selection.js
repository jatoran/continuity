// Projection-owned touch selection.
//
// The platform's long-press selection hit-tests the invisible textarea, whose
// layout cannot match the projection: folded Markdown markers make projected
// text shorter than source, headings render at up to 1.45em, and the projection
// box is 14px narrower. A textarea has no per-line font-size, so the two
// layouts can never be made to agree — the OS therefore selects the character
// under the finger in *textarea* coordinates and Continuity repaints that
// source range at *projection* coordinates, which is somewhere else entirely.
//
// So touch selection is taken over here: long-press selects the word under the
// finger through the projection, and the drag extends it through the
// projection. Plain finger drags are untouched and still scroll.
//
// The press is timed rather than driven by `contextmenu`. Android Chrome raises
// no `contextmenu` for a long-press on a textarea — it opens the platform
// selection UI directly — so a `contextmenu`-only claim never fires on a phone.
// The timer is deliberately shorter than the platform's own threshold so the
// claim lands first and `selectstart` can be suppressed before the platform
// commits to a selection.

import { projectionPositionAtPoint } from "./projection.js";

const LONG_PRESS_SLOP_SQUARED = 100;
const LONG_PRESS_DELAY_MS = 320;
// How long after the finger leaves the platform may still be running its own
// selection drag. Observed at roughly 600ms on Android; guard a little past it.
const PLATFORM_GUARD_MS = 900;

/** Own one long-press-and-drag touch selection against projected geometry. */
export class TouchSelectionGesture {
  #anchorRange;
  #claimedAt = 0;
  #hasClaimed = false;
  #isActive = false;
  #isPointerDown = false;
  #onClaim;
  #origin;
  #pointerId;
  #timer;

  /** Record where a finger landed and arm the long-press claim. */
  begin(event, onClaim) {
    this.cancel();
    this.#hasClaimed = false;
    if (event.pointerType !== "touch") return;
    this.#isPointerDown = true;
    this.#pointerId = event.pointerId;
    this.#origin = { clientX: event.clientX, clientY: event.clientY };
    this.#onClaim = onClaim;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      if (this.#origin) this.#onClaim?.(this.#origin);
    }, LONG_PRESS_DELAY_MS);
  }

  /** Pointer this gesture owns, for capture while a selection drag runs. */
  get pointerId() {
    return this.#pointerId;
  }

  get isActive() {
    return this.#isActive;
  }

  /** Whether a finger is still down, claimed or not. */
  get isPointerDown() {
    return this.#isPointerDown;
  }

  /**
   * Whether this touch ever produced a projection-owned selection — still true
   * after the platform cancels the pointer to run its own long-press selection.
   * That platform selection is hit-tested against the invisible textarea, so
   * adopting it would replace a correct selection with one tens of characters
   * away. The flag survives until the next finger lands.
   */
  get hasClaimed() {
    if (!this.#hasClaimed) return false;
    if (this.#isPointerDown) return true;
    // Bounded: an unbounded guard would refuse every later platform caret move —
    // a dragged handle, a spacebar cursor slide, an autocorrect replacement —
    // for the rest of the editor's life. It only ever gates *adoption* of the
    // textarea's own answer (`applyNativeSelection`); a host `setSelections` or
    // `revealRange` writes the engine first and pushes the result into the
    // textarea, so it is applied during this window rather than dropped.
    if (Date.now() - this.#claimedAt < PLATFORM_GUARD_MS) return true;
    this.#hasClaimed = false;
    return false;
  }

  /**
   * Claim the press at a point and return the projected position to anchor on,
   * or null when the projection cannot resolve a glyph there.
   */
  claimAt(point, projection) {
    if (!this.#origin) return null;
    const position = projectionPositionAtPoint(projection, point.clientX, point.clientY);
    if (!position) return null;
    this.#isActive = true;
    this.#hasClaimed = true;
    this.#claimedAt = Date.now();
    // Held until the word-select result replaces it with the full word range.
    this.#anchorRange = { start: position, end: position };
    return position;
  }

  /**
   * Adopt the word the long-press selected as the anchor *range*. A single
   * anchor point is not enough: the finger lands in the middle of a word, so
   * anchoring at one end and letting the head follow the finger collapses the
   * selection to the part of the word between them — the highlight appears
   * beside the finger rather than under it, and cannot grow until the finger
   * passes the far edge.
   */
  anchorToRange(start, end) {
    if (this.#isActive) this.#anchorRange = { start, end };
  }

  /** Extend a claimed selection to the projected glyph under the finger. */
  extend(event, projection) {
    if (!this.#isActive || !this.#anchorRange) return null;
    const head = projectionPositionAtPoint(projection, event.clientX, event.clientY);
    if (!head) return null;
    // Keep the platform from scrolling or re-opening its own selection while
    // this gesture owns the finger.
    if (event.cancelable) event.preventDefault();
    const { start, end } = this.#anchorRange;
    // Grow from whichever edge the finger has passed, and hold the whole word
    // while the finger is still inside it.
    if (comparePositions(head, start) < 0) return { anchor: end, head };
    if (comparePositions(head, end) > 0) return { anchor: start, head };
    return { anchor: start, head: end };
  }

  /** A finger that travels before the press matures is a pan, not a selection. */
  disarmIfWandered(event) {
    if (this.#isActive || !this.#origin || this.#timer === undefined) return;
    const horizontal = event.clientX - this.#origin.clientX;
    const vertical = event.clientY - this.#origin.clientY;
    if (horizontal * horizontal + vertical * vertical > LONG_PRESS_SLOP_SQUARED) {
      this.cancel();
    }
  }

  cancel() {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    this.#isActive = false;
    this.#isPointerDown = false;
    this.#anchorRange = undefined;
    this.#origin = undefined;
    this.#onClaim = undefined;
    this.#pointerId = undefined;
  }
}

/** Document order of two source positions. */
function comparePositions(left, right) {
  return left.line - right.line || left.byteInLine - right.byteInLine;
}
