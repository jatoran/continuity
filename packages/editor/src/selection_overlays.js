import { appendVisualCarets } from "./visual_carets.js";
import { appendDecorations } from "./decorations.js";
import { computeRangeRects, orderedRange, placeOverlayRect, positionOrder } from "./overlay_rects.js";

/** Paint host decorations, visible selection ranges, and carets from one snapshot. */
export function renderSelectionOverlays(container, projection, snapshot) {
  const containerBounds = container.getBoundingClientRect();
  const fragment = document.createDocumentFragment();
  // Decorations first: a selection over a decorated range has to stay legible
  // as a selection, and both are translucent.
  appendDecorations(fragment, projection, container, containerBounds);
  appendVisualSelections(fragment, projection, snapshot, containerBounds);
  const primaryCaret = appendVisualCarets(fragment, projection, snapshot, containerBounds);
  container.replaceChildren(fragment);
  return { primaryCaret };
}

function appendVisualSelections(fragment, projection, snapshot, containerBounds) {
  snapshot.selections.forEach((selection, selectionIndex) => {
    if (isCollapsed(selection)) return;
    const ordered = orderedRange(selection.anchor, selection.head);
    computeRangeRects(projection, ordered, true).forEach((bounds) => appendSelectionRect(
      fragment, bounds, selectionIndex, containerBounds,
    ));
  });
}

function appendSelectionRect(fragment, bounds, selectionIndex, containerBounds) {
  const highlight = document.createElement("span");
  highlight.className = "visual-selection";
  highlight.dataset.selectionIndex = String(selectionIndex);
  placeOverlayRect(highlight, bounds, containerBounds);
  fragment.append(highlight);
}

function isCollapsed(selection) {
  return positionOrder(selection.anchor, selection.head) === 0;
}
