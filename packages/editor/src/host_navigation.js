// Host-facing selection and viewport API.
//
// Kept beside the element rather than inside it so the class body stays under
// the file cap. These are the operations a host drives directly, and each one
// has to leave the editor's own bookkeeping consistent: the tracked selection
// key, and a scheduled render.

import { inputSelectionKey } from "./input_sync.js";
import { applyPublicSelections, restorePublicScroll, revealPublicRange } from "./navigation.js";

/** Replace every engine selection, optionally revealing the primary one. */
export function setHostSelections(ctx, editor, selections, options) {
  const snapshot = applyPublicSelections(editor, ctx.input, selections);
  const primary = snapshot.selections[0];
  if (options.reveal !== false && primary) {
    ctx.requestReveal({ start: primary.head, end: primary.head }, "nearest");
  }
  ctx.setSelectionKey(inputSelectionKey(ctx.input));
  ctx.schedule();
  // A host-driven selection is as selectable as a finger-drawn one, so it gets
  // the same clipboard affordance.
  ctx.updateSelectionActions?.();
  return snapshot;
}

/** Reveal a source range and make it the primary selection. */
export function revealHostRange(ctx, editor, range, options) {
  const snapshot = revealPublicRange(editor, ctx.input, range);
  const primary = snapshot.selections[0];
  if (primary) {
    ctx.requestReveal(
      { start: primary.anchor, end: primary.head },
      options.align === "center" ? "center" : "nearest",
    );
  }
  ctx.setSelectionKey(inputSelectionKey(ctx.input));
  ctx.schedule();
}

/** The viewport of whichever element currently owns scrolling. */
export function hostScrollState(ctx) {
  const view = ctx.scroller() ?? ctx.input;
  return { top: view?.scrollTop ?? 0, left: view?.scrollLeft ?? 0 };
}

/** Restore a captured viewport and repaint the projection against it. */
export function restoreHostScroll(ctx, snapshot, state) {
  restorePublicScroll(
    ctx.scroller() ?? ctx.input, ctx.projection, ctx.affordances, ctx.carets, snapshot, state,
  );
  // A restore that lands on the current offset raises no native scroll event,
  // so the visible-window probe is driven here rather than left to one.
  ctx.probeViewport?.();
}
