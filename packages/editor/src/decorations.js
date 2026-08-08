// Host range decorations.
//
// A host cannot reach into the shadow root to paint anything, and it cannot
// find offscreen matches either: the projection realizes only the viewport
// window, so a browser find, a host-side DOM walk, and CSS Highlights all see a
// few dozen lines of a document that may hold thousands. Decorations are
// therefore expressed in source coordinates and painted here, in the same pass
// and the same geometry as the selection overlay.
//
// Ranges are positions, not anchors: they do not follow edits. A host that
// decorates search matches re-runs its search on `continuity-change` and sets
// the new ranges, which is what it would do anyway.

import { computeRangeRects, orderedRange, placeOverlayRect } from "./overlay_rects.js";

const decorationStates = new WeakMap();
const DECORATION_ID = /^[a-zA-Z][a-zA-Z0-9_-]*$/u;

/**
 * Replace one named decoration set. An empty list removes the set, so a host
 * clears by assigning `[]` as readily as by calling `clearDecorations`.
 * Returns the number of ranges retained.
 */
export function setDecorationRanges(container, id, ranges) {
  const key = requireDecorationId(id);
  const normalized = normalizeRanges(ranges);
  const state = decorationStates.get(container) ?? new Map();
  if (normalized.length === 0) {
    state.delete(key);
  } else {
    state.set(key, normalized);
  }
  decorationStates.set(container, state);
  return normalized.length;
}

/** Remove one decoration set, or every set when no id is given. */
export function clearDecorationRanges(container, id) {
  const state = decorationStates.get(container);
  if (!state) return;
  if (id === undefined) state.clear();
  else state.delete(requireDecorationId(id));
}

/** Ids of every set currently painting, in the order they were first set. */
export function listDecorationIds(container) {
  return [...(decorationStates.get(container)?.keys() ?? [])];
}

/** Paint every decoration set against the current projection geometry. */
export function appendDecorations(fragment, projection, container, containerBounds) {
  const state = decorationStates.get(container);
  if (!state || state.size === 0) return;
  for (const [id, ranges] of state) {
    for (const range of ranges) {
      for (const bounds of computeRangeRects(projection, range, false)) {
        fragment.append(decorationRect(id, bounds, containerBounds));
      }
    }
  }
}

function decorationRect(id, bounds, containerBounds) {
  const rect = document.createElement("span");
  rect.className = "decoration";
  rect.dataset.decorationId = id;
  rect.setAttribute("part", `decoration decoration-${id}`);
  // The id is a validated CSS identifier, so the per-set property name is safe
  // to build here — this is what lets one set be themed without the host being
  // able to reach the rule that draws it.
  rect.style.background = `var(--continuity-decoration-${id}, var(--continuity-decoration))`;
  placeOverlayRect(rect, bounds, containerBounds);
  return rect;
}

function requireDecorationId(id) {
  const key = String(id);
  if (!DECORATION_ID.test(key)) {
    throw new RangeError(
      `decoration id ${JSON.stringify(key)} must be a CSS identifier: a letter followed by letters, digits, hyphens, or underscores`,
    );
  }
  return key;
}

function normalizeRanges(ranges) {
  if (ranges === undefined || ranges === null) return [];
  if (!Array.isArray(ranges)) {
    throw new TypeError("decoration ranges must be an array of { start, end } source ranges");
  }
  const normalized = [];
  for (const range of ranges) {
    const start = normalizePosition(range?.start);
    const end = normalizePosition(range?.end);
    if (!start || !end) continue;
    const ordered = orderedRange(start, end);
    if (ordered.start.line === ordered.end.line
        && ordered.start.byteInLine === ordered.end.byteInLine) {
      continue;
    }
    normalized.push(ordered);
  }
  return normalized;
}

function normalizePosition(position) {
  const line = position?.line;
  const byteInLine = position?.byteInLine;
  if (!Number.isSafeInteger(line) || line < 0) return null;
  if (!Number.isSafeInteger(byteInLine) || byteInLine < 0) return null;
  return { line, byteInLine };
}
