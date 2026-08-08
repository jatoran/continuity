import { selectedLines } from "./coordinates.js";
import { renderCodeAffordances } from "./code_affordances.js";
import {
  createPlainPresentation,
  isProjectionStructuralDirty,
  projectionActiveLines,
  projectionDetailedRange,
  renderActiveSourceLines,
  renderProjection,
  renderProjectionRange,
  renderProjectionViewport,
  shouldRefreshProjection,
} from "./projection.js";

/**
 * Which lines reveal raw source. Normally that follows the selection, but a
 * finger dragging a selection must not move the glyphs underneath itself:
 * revealing markers on the line the head just entered (and re-folding the one
 * it left) re-lays-out the text mid-gesture, so the finger ends up over
 * different characters than it was aimed at. Freeze the reveal for the length
 * of the drag and let it settle when the finger lifts.
 */
function resolveActiveLines(context, snapshot, input) {
  const frozen = context.shouldFreezeSourceReveal?.() ? projectionActiveLines(context.projection) : null;
  return frozen ?? selectedLines(snapshot.text, input);
}

/** Run the incremental keystroke pass; return false to request a full render. */
export function renderFastPresentation(context) {
  const { snapshot, input, projection, synchronizer, edits } = context;
  const activeLines = resolveActiveLines(context, snapshot, input);
  if (!renderActiveSourceLines(projection, snapshot, activeLines, edits)) {
    return false;
  }
  const shouldMeasureViewport = edits.some((edit) => (
    edit.startLine !== edit.endLine || edit.insertedText.includes("\n")
  ));
  renderProjectionViewport(projection, synchronizer.scroller, shouldMeasureViewport);
  synchronizer.afterRender(snapshot);
  return true;
}

/** Run the idle/full presentation pass, preferring a viewport-scoped engine report. */
export function renderEditorPresentation(context) {
  const snapshot = context.editor.snapshot();
  const activeLines = resolveActiveLines(context, snapshot, context.input);
  const detailedRange = projectionDetailedRange(context.projection);
  if (context.syntax === "markdown" && !shouldRefreshProjection(context.projection, activeLines)) {
    context.synchronizer.afterRender(snapshot);
    return;
  }
  if (context.syntax === "markdown"
      && !isProjectionStructuralDirty(context.projection)
      && detailedRange.end > detailedRange.start) {
    const patch = context.editor.presentationRange(detailedRange.start, detailedRange.end);
    if (renderProjectionRange(
      context.projection, context.synchronizer.scroller, snapshot, patch, activeLines,
    )) {
      context.synchronizer.afterRender(snapshot);
      return;
    }
  }
  const presentation = context.syntax === "plain"
    ? createPlainPresentation(snapshot.text)
    : context.editor.presentation();
  renderProjection(
    context.projection, context.synchronizer.scroller, snapshot, presentation, activeLines,
  );
  renderCodeAffordances(
    context.affordances,
    context.projection,
    snapshot,
    presentation,
    activeLines,
    context.copyCode,
  );
  context.synchronizer.afterRender(snapshot);
}
