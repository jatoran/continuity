// Whether a touch is allowed to raise the soft keyboard - and whether it is
// allowed to take one away.
//
// Keyboard visibility on Android is not a function of DOM focus. Dismissing the
// keyboard with the system back gesture hides the IME *without* blurring, so the
// editor's textarea is still the active element afterwards - and Chrome then
// re-raises the IME for any touch that resolves against that same focused
// editable. A long-press selection inside the editor is exactly such a touch, so
// selecting a word brought the keyboard back up over the words being selected.
//
// No focus policy can close that, because focus never moves. The editor already
// refuses to focus anything on the touch path until a tap resolves; that is
// necessary and not sufficient. `inputmode="none"` is the other half: Chrome
// will not raise the IME for a focused field in that state however the touch
// resolved, which is the one property focus bookkeeping cannot provide.
// (`virtualkeyboardpolicy` is the attribute built for this and is not usable
// here: it applies only to `contenteditable`, and this surface is a
// `<textarea>`.)
//
// The attribute is not symmetric, which is the rest of this file. On a field
// focused *with the IME up*, `inputmode="none"` does not refuse a future raise -
// it dismisses the keyboard that is already there. Applying it to every
// selection gesture therefore fixed the reader's case by breaking the writer's.
// So the gate is not a reflex: it consults `soft_keyboard_policy.js`, which
// tracks whether a keyboard is actually up, and closes only where there is none
// to lose.

import { isTouchScrolling } from "./scroll_surface.js";
import { KEYBOARD_DOWN, KEYBOARD_RAISED, createSoftKeyboardPolicy } from "./soft_keyboard_policy.js";

/**
 * Own one editor's input mode and the keyboard state behind it.
 *
 * The gate is per-editor rather than per-module because typing intent is: two
 * editors on a page have their own resolved gestures. The viewport it observes
 * is a window-wide fact, and is read from the global at construction so a test
 * can hand in its own - a headless browser has no IME, so an injected viewport
 * is the only way the raised-keyboard transitions can be driven at all.
 *
 * @param {HTMLTextAreaElement} input
 * @param {{policy?: object, viewport?: EventTarget & {height: number}}} [options]
 */
export function createSoftKeyboardGate(input, options = {}) {
  const policy = createSoftKeyboardPolicy(options.policy);
  const viewport = "viewport" in options
    ? options.viewport
    : globalThis.visualViewport ?? null;
  // Raising re-enters focus, which blurs first; that blur is this gate's own
  // and must not be read as the keyboard going away.
  let isRaising = false;

  /**
   * Hold the input surface where a focused textarea cannot raise the keyboard.
   *
   * A no-op on a fine pointer, which is what leaves desktop untouched: mouse and
   * pen already focus on pointerdown, and `inputmode` means nothing to a physical
   * keyboard. The gate is deliberately not re-evaluated when the primary pointer
   * changes mid-session - a tablet leaving its dock - because the state it would
   * be leaving behind has no effect on the pointer it would be leaving it for.
   */
  const closeGate = () => {
    if (input && isTouchScrolling()) input.setAttribute("inputmode", "none");
  };

  // Closed at construction. Nothing has been touched yet, so no keyboard can be
  // up, which makes this the one place the gate closes without asking.
  closeGate();
  // Seed the height baseline now rather than from the first resize. Occlusion is
  // relative — there is no absolute "unoccluded" height for a window — so a
  // policy whose first sample is also its first resize has nothing to compare
  // against and reads a keyboard arriving as no keyboard at all.
  if (viewport) policy.observeViewportHeight(viewport.height);

  return {
    /** The viewport whose resizes feed this gate, for the listener wiring. */
    viewport,

    /**
     * This gesture is selection, not typing.
     *
     * Applied wherever a gesture resolves as selection - a matured long-press,
     * an adjust-handle grab - so a gesture that begins as selection can never
     * end in a keyboard. It closes the gate only while the keyboard is down;
     * with one up, closing it would be the dismissal rather than the
     * prevention, and a reader adjusting a selection mid-sentence would lose
     * the keyboard they were typing on.
     *
     * Every such gesture also produces a fresh selection, so the raise-only tap
     * that selection is entitled to has not been spent yet.
     */
    holdForSelection() {
      policy.releaseSelectionPreservation();
      if (!policy.shouldGateForSelection()) return false;
      closeGate();
      return true;
    },

    /**
     * Lift the gate and focus, inside the caller's trusted turn.
     *
     * Chrome requires user activation to show the IME and reads the input mode
     * at the moment it decides, so both halves have to land before that turn
     * returns. Every caller is therefore a resolved tap or an explicit insert,
     * never something deferred behind a promise or a frame.
     */
    raiseForTyping(focusOptions = { preventScroll: true }) {
      if (!input) return;
      const wasGated = input.getAttribute("inputmode") === "none";
      input.removeAttribute("inputmode");
      policy.noteTypingIntent();
      isRaising = true;
      try {
        // Re-entering focus is what re-arms the browser's own request to show
        // the IME. Focus may never have left - the back gesture hides the
        // keyboard without blurring it - and `focus()` on the element that
        // already has focus is a no-op, so lifting the mode alone can leave the
        // keyboard down on the very tap that asked for it. This cannot flicker,
        // and only because the gate was closed: a closed gate means no keyboard
        // was showing to be taken away.
        if (wasGated && input.getRootNode().activeElement === input) input.blur();
        input.focus(focusOptions);
      } finally {
        isRaising = false;
      }
    },

    /**
     * Claim the one tap that may raise the keyboard while leaving the selection
     * it landed in exactly as it was. See `claimSelectionPreservation`.
     */
    claimSelectionPreservation(selectionKey) {
      return policy.claimSelectionPreservation(selectionKey);
    },

    /**
     * The surface lost focus, so the keyboard is definitely gone. Re-arm the
     * gate with it: the next touch resolves against an unfocused editable, and
     * leaving the mode lifted would let that touch raise a keyboard the reader
     * never asked for.
     */
    noteBlur() {
      if (isRaising) return policy.visibility;
      const state = policy.noteDismissed();
      closeGate();
      return state;
    },

    /**
     * Fold the viewport's current height into the tracked state.
     *
     * The gate is re-armed only on the fall from a raised keyboard, never on
     * every settled resize: a viewport that shrinks and grows for a collapsing
     * URL bar must not close a gate a tap has just opened, or the keyboard that
     * tap asked for dies on arrival.
     */
    observeViewport() {
      if (!viewport) return policy.visibility;
      const before = policy.visibility;
      const after = policy.observeViewportHeight(viewport.height);
      if (before === KEYBOARD_RAISED && after === KEYBOARD_DOWN) closeGate();
      return after;
    },
  };
}
