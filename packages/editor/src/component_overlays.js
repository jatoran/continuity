// Chrome painted over the projection.
//
// Three things share one lifetime and one repaint: the touch selection action
// bar, the touch selection adjust handles, and host range decorations. All
// three are anchored to painted selection geometry rather than to engine
// coordinates, so they have to settle after the overlay pass and be re-placed
// by it — which is why they are assembled together here instead of each being
// wired separately into the element.

import { createSelectionActions } from "./selection_actions.js";
import { createSelectionHandles } from "./selection_handles.js";
import { clearDecorationRanges, setDecorationRanges } from "./decorations.js";
import { renderSelectionOverlays } from "./selection_overlays.js";
import { commitProjectedSelection, runSelectionAction } from "./component_pointer.js";
import { orderedRange } from "./overlay_rects.js";
import { projectionPositionAtPoint } from "./projection.js";
import { isTouchScrolling } from "./scroll_surface.js";
import { applySoftKeyboardGate } from "./soft_keyboard.js";

/** Build the overlay chrome and return the controller the element drives. */
export function createEditorOverlays(ctx, dom) {
  const isEnabled = () => isTouchScrolling()
    && !ctx.isComposing()
    && !ctx.touchSelection.isActive;
  const actions = createSelectionActions(dom, {
    isEnabled,
    canPaste: () => Boolean(navigator.clipboard?.readText),
    captureSelection: () => ctx.input.value.slice(ctx.input.selectionStart, ctx.input.selectionEnd),
    run: (action, text) => runSelectionAction(ctx, action, text),
  });
  // Frozen for the length of one handle drag so the selection can be dragged
  // through and past its own anchor without the ends swapping under the finger.
  let dragAnchor = null;
  const handles = createSelectionHandles(dom, {
    isEnabled,
    onDragStart: (edge) => {
      // Adjusting a selection is not typing. The handle is grabbed while the
      // textarea already holds focus, so without closing the gate here the grab
      // reaches Chrome as one more touch against a focused editable.
      applySoftKeyboardGate(ctx.input);
      dragAnchor = frozenAnchor(ctx, edge);
      actions.hide();
    },
    extendEdge: (clientX, clientY) => {
      extendSelectionEdge(ctx, dragAnchor, clientX, clientY);
      handles.update();
    },
    onDragEnd: () => {
      dragAnchor = null;
      actions.update();
      handles.update();
    },
  });

  const repaint = () => {
    const snapshot = ctx.editor()?.snapshot();
    if (snapshot) renderSelectionOverlays(dom.carets, ctx.projection, snapshot);
  };

  return {
    /** Whether a handle drag owns the pointer; source reveal freezes for it. */
    isAdjusting: () => handles.isDragging,
    update() {
      actions.update();
      handles.update();
    },
    hide() {
      actions.hide();
      handles.hide();
    },
    offerForCaret() {
      actions.offerForCaret();
      handles.update();
    },
    setDecorations(id, ranges) {
      const count = setDecorationRanges(dom.carets, id, ranges);
      repaint();
      return count;
    },
    clearDecorations(id) {
      clearDecorationRanges(dom.carets, id);
      repaint();
    },
  };
}

/** The selection end a handle drag pivots around, read once at grab time. */
function frozenAnchor(ctx, edge) {
  const selection = ctx.editor()?.snapshot().selections[0];
  if (!selection) return null;
  const ordered = orderedRange(selection.anchor, selection.head);
  return edge === "start" ? ordered.end : ordered.start;
}

/** Move the dragged end to the projected glyph the handle is aimed at. */
function extendSelectionEdge(ctx, anchor, clientX, clientY) {
  if (!anchor || !ctx.editor()) return;
  const head = projectionPositionAtPoint(ctx.projection, clientX, clientY);
  if (!head) return;
  commitProjectedSelection(ctx, { anchor, head });
  renderSelectionOverlays(ctx.carets, ctx.projection, ctx.editor().snapshot());
}
