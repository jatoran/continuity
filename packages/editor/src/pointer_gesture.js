import { isMarkdownCheckboxAt, projectionPositionAtPoint } from "./projection.js";
import { applySelectionToInput, selectionFromInput } from "./coordinates.js";
import { inputSelectionKey } from "./input_sync.js";

/** Focus and capture one trusted mouse/pen pointer for projection-owned dragging. */
export function applyPrimaryPointerCapture(input, event, surface = input) {
  if (event.pointerType === "touch") {
    // A finger must keep its native contract until the gesture resolves: a
    // drag pans, while only a completed tap focuses. Preventing the default,
    // capturing, or focusing here interrupts scrolling and can flash the soft
    // keyboard before the browser has classified the gesture.
    return;
  }
  event.preventDefault();
  input.focus({ preventScroll: true });
  if (event.isTrusted && surface.setPointerCapture) surface.setPointerCapture(event.pointerId);
}

/** Commit one projection-mapped range to the semantic input and engine. */
export function applyProjectedPointerSelection(editor, input, selection) {
  if (!editor || !selection) return null;
  const snapshot = editor.snapshot();
  applySelectionToInput(snapshot.text, selection, input);
  editor.setSelections([selectionFromInput(snapshot.text, input)]);
  return inputSelectionKey(input);
}

/** Complete a primary click without Ctrl/Command multi-selection semantics. */
export function handleUnmodifiedPointerClick(
  event, gesture, editor, input, onSelectionChanged, onCheckbox,
) {
  if (event.ctrlKey || event.metaKey) return false;
  if (!event.shiftKey && applyMultiClickSelection(editor, gesture)) {
    onSelectionChanged();
  } else if (gesture.clickCount === 1 && !event.shiftKey
      && isMarkdownCheckboxAt(input.value, input.selectionStart)) {
    onCheckbox();
  }
  return true;
}

function applyMultiClickSelection(editor, gesture) {
  if (!editor || !gesture.selection || gesture.clickCount < 2) return false;
  editor.executeCommand(gesture.clickCount >= 3 ? "editor.select_line" : "editor.select_word");
  return true;
}

/** Own one physical pointer gesture without conflating it with synthetic clicks. */
export class ProjectionPointerGesture {
  #anchor;
  #origin;
  #pointerType;
  #position;
  #selectionsBeforePointer;
  #wasDragged = false;

  begin(event, projection, selections) {
    this.#origin = { clientX: event.clientX, clientY: event.clientY };
    this.#pointerType = event.pointerType;
    this.#position = projectionPositionAtPoint(projection, event.clientX, event.clientY);
    this.#anchor = event.shiftKey && selections[0] ? selections[0].anchor : this.#position;
    this.#selectionsBeforePointer = event.ctrlKey || event.metaKey ? selections : undefined;
    this.#wasDragged = false;
  }

  move(event, projection) {
    if ((event.buttons & 1) === 0 || !this.#origin) return null;
    const horizontal = event.clientX - this.#origin.clientX;
    const vertical = event.clientY - this.#origin.clientY;
    const slopSquared = event.pointerType === "touch" ? 576 : 9;
    this.#wasDragged ||= horizontal * horizontal + vertical * vertical > slopSquared;
    // A moving finger is a scroll (or a native handle drag), never a
    // mouse-style drag selection; selecting during it threw the caret to
    // wherever the scroll gesture started. Travel is still recorded so a
    // defensive trailing click cannot turn a pan into a tap.
    if (event.pointerType === "touch") return null;
    if (!this.#wasDragged || !this.#anchor) return null;
    const head = projectionPositionAtPoint(projection, event.clientX, event.clientY);
    if (!head) return null;
    event.preventDefault();
    return { anchor: this.#anchor, head };
  }

  cancel() {
    this.#anchor = undefined;
    this.#origin = undefined;
    this.#pointerType = undefined;
    this.#position = undefined;
    this.#selectionsBeforePointer = undefined;
    this.#wasDragged = false;
  }

  complete(event) {
    const horizontal = event.clientX - (this.#origin?.clientX ?? event.clientX);
    const vertical = event.clientY - (this.#origin?.clientY ?? event.clientY);
    // Finger taps jitter well past the 3px mouse slop; rejecting them dropped
    // the projected mapping and let the raw-layout caret placement win.
    const pointerType = this.#pointerType ?? event.pointerType;
    const slopSquared = pointerType === "touch" ? 576 : 9;
    const isCapturedClick = Boolean(this.#origin)
      && horizontal * horizontal + vertical * vertical <= slopSquared;
    const result = {
      selection: !this.#wasDragged && isCapturedClick && this.#position
        ? { anchor: event.shiftKey ? this.#anchor : this.#position, head: this.#position }
        : null,
      isTap: !this.#wasDragged && isCapturedClick,
      clickCount: Math.max(1, Math.min(3, event.detail || 1)),
      pointerType,
      selectionsBeforePointer: this.#selectionsBeforePointer,
    };
    this.#anchor = undefined;
    this.#origin = undefined;
    this.#pointerType = undefined;
    this.#position = undefined;
    this.#selectionsBeforePointer = undefined;
    this.#wasDragged = false;
    return result;
  }
}
