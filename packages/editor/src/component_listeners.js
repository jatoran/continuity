// Browser event wiring for the editor element.
//
// Split from the element itself so the class body stays under the file cap and
// this file owns one concern: which DOM node each listener belongs on. That is
// not incidental — pointer listeners sit on the frame so both the textarea and
// the touch shield feed them, keyboard and clipboard listeners stay on the
// textarea because only it has a selection and a value, and `selectionchange`
// is document-scoped because no element raises it for a text control.

import { hideCodeAffordances } from "./code_affordances.js";
import { handleCopy } from "./input_events.js";
import { observeTouchPointer } from "./scroll_surface.js";

/**
 * Attach every listener the editor needs. `handlers` carries the element's
 * bound callbacks; `hooks` the small closures over its private state.
 */
export function installEditorListeners(dom, handlers, hooks, options) {
  const { affordances, frame, input, shield, softKeyboard } = dom;
  for (const [type, handler] of Object.entries({
    beforeinput: handlers.onBeforeInput,
    input: handlers.onInput,
    compositionstart: handlers.onCompositionStart,
    compositionend: handlers.onCompositionEnd,
    keydown: handlers.onKeyDown,
    selectstart: handlers.onSelectStart,
    select: handlers.onSelection,
    scroll: handlers.onScroll,
    paste: handlers.onPaste,
    cut: handlers.onCut,
    drop: handlers.onDrop,
  })) {
    input.addEventListener(type, handler, options);
  }
  // Pointer events land on the frame: the finger may be on the shield and the
  // mouse on the textarea, and both must reach the same gesture handling.
  for (const [type, handler] of Object.entries({
    pointerdown: handlers.onPointerDown,
    pointermove: handlers.onPointerMove,
    pointercancel: handlers.onPointerCancel,
    pointerup: handlers.onPointerUp,
    click: handlers.onClick,
    contextmenu: handlers.onContextMenu,
  })) {
    frame.addEventListener(type, handler, options);
  }
  frame.addEventListener("pointerleave", (event) => {
    if (!affordances.contains(event.relatedTarget)) hideCodeAffordances(affordances);
  }, options);
  input.addEventListener("copy", (event) => {
    const snapshot = hooks.snapshot();
    if (snapshot) handleCopy(event, snapshot);
  }, options);
  input.addEventListener("dragover", (event) => event.preventDefault(), options);
  // `select` never fires for a collapsed caret move, so platform-driven caret
  // repositioning reaches the engine only through `selectionchange`.
  input.ownerDocument.addEventListener("selectionchange", handlers.onSelectionChange, options);
  shield.addEventListener("scroll", handlers.onScroll, options);
  // Non-passive: a passive listener cannot refuse the scroll, and refusing it is
  // the only way a selection drag survives its first millimetre on a scroller.
  const active = { ...options, passive: false };
  shield.addEventListener("touchmove", handlers.onTouchMove, active);
  input.addEventListener("touchmove", handlers.onTouchMove, active);
  installSoftKeyboardListeners(input, softKeyboard, options);
  observeTouchPointer(handlers.onTouchPointerChange);
}

/**
 * Feed the keyboard gate the two events that say anything about the IME.
 *
 * They go straight to the gate rather than through the element's handler bag
 * because neither is editor state: a blur means the keyboard is gone whatever
 * the editor was doing, and the visual viewport is a window-wide fact the
 * element has no other reason to hold. `visualViewport` is the only signal
 * Android gives for a keyboard that appears and disappears without touching
 * focus, which is exactly the case the gate exists to tell apart.
 */
function installSoftKeyboardListeners(input, softKeyboard, options) {
  if (!softKeyboard) return;
  input.addEventListener("blur", () => softKeyboard.noteBlur(), options);
  softKeyboard.viewport?.addEventListener("resize", () => softKeyboard.observeViewport(), options);
}
