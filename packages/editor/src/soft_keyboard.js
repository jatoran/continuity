// Whether a touch is allowed to raise the soft keyboard.
//
// Keyboard visibility on Android is not a function of DOM focus. Dismissing the
// keyboard with the system back gesture hides the IME *without* blurring, so the
// editor's textarea is still the active element afterwards — and Chrome then
// re-raises the IME for any touch that resolves against that same focused
// editable. A long-press selection inside the editor is exactly such a touch, so
// selecting a word brought the keyboard back up over the words being selected.
//
// No focus policy can close that, because focus never moves. The editor already
// refuses to focus anything on the touch path until a tap resolves; that is
// necessary and not sufficient. `inputmode="none"` is the other half: Chrome
// will not raise the IME for a focused field in that state however the touch
// resolved, which is the one property focus bookkeeping cannot provide. So the
// input surface is held there on a coarse pointer and lifted only where the
// editor has already decided a touch meant "type here".
//
// `virtualkeyboardpolicy` is the attribute built for this and is not usable
// here: it applies only to `contenteditable` elements, and this input surface is
// a `<textarea>`.

import { isTouchScrolling } from "./scroll_surface.js";

/**
 * Hold the input surface where a focused textarea cannot raise the keyboard.
 *
 * Applied when the DOM is built, and re-applied wherever a gesture resolves as
 * selection rather than typing, so a gesture that begins as selection can never
 * end in a keyboard.
 *
 * A no-op on a fine pointer, which is what leaves desktop untouched: mouse and
 * pen already focus on pointerdown, and `inputmode` means nothing to a physical
 * keyboard. The gate is deliberately not re-evaluated when the primary pointer
 * changes mid-session — a tablet leaving its dock — because the state it would
 * be leaving behind has no effect on the pointer it would be leaving it for.
 */
export function applySoftKeyboardGate(input) {
  if (input && isTouchScrolling()) input.setAttribute("inputmode", "none");
}

/**
 * Lift the gate and focus, inside the caller's trusted turn.
 *
 * Chrome requires user activation to show the IME and reads the input mode at
 * the moment it decides, so both halves have to land before that turn returns.
 * Every caller is therefore a resolved tap or an explicit insert, never
 * something deferred behind a promise or a frame.
 */
export function raiseSoftKeyboard(input, options = { preventScroll: true }) {
  if (!input) return;
  const wasGated = input.getAttribute("inputmode") === "none";
  input.removeAttribute("inputmode");
  // Re-entering focus is what re-arms the browser's own request to show the IME.
  // Focus may never have left — the back gesture hides the keyboard without
  // blurring it — and `focus()` on the element that already has focus is a
  // no-op, so lifting the mode alone can leave the keyboard down on the very tap
  // that asked for it. This cannot flicker, and only because the gate was up: a
  // raised gate means no keyboard was showing to be taken away.
  if (wasGated && input.getRootNode().activeElement === input) input.blur();
  input.focus(options);
}
