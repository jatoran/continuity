import { applySelectionToInput } from "./coordinates.js";
import {
  renderProjectionViewport,
  synchronizeProjectionScroll,
} from "./projection.js";
import { renderSelectionOverlays } from "./selection_overlays.js";

/** Apply public engine selections to the semantic textarea. */
export function applyPublicSelections(editor, input, selections) {
  editor.setSelections(selections);
  const snapshot = editor.snapshot();
  if (snapshot.selections[0]) {
    applySelectionToInput(snapshot.text, snapshot.selections[0], input);
  }
  return snapshot;
}

/** Make a source range the primary selection so the projection can reveal it. */
export function revealPublicRange(editor, input, range) {
  const selection = { anchor: range.start, head: range.end, kind: "caret" };
  return applyPublicSelections(editor, input, [selection]);
}

/** Restore a validated textarea viewport and its visual projection. */
export function restorePublicScroll(input, projection, affordances, carets, snapshot, state) {
  const top = Number.isFinite(state?.top) ? Math.max(0, state.top) : 0;
  const left = Number.isFinite(state?.left) ? Math.max(0, state.left) : 0;
  input.scrollTop = top;
  input.scrollLeft = left;
  synchronizeProjectionScroll(input, projection, affordances);
  renderProjectionViewport(projection, input);
  renderSelectionOverlays(carets, projection, snapshot);
}
