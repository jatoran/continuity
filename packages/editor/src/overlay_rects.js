// Client-space rectangles for one source range, measured through the rendered
// projection. Shared by the selection overlay and by host range decorations so
// both resolve folded markers, replaced list bullets, and scaled headings the
// same way — a range is only ever as accurate as the geometry it is measured
// against, and there is exactly one such geometry.

import { projectionDetailedRange, projectionLineLayout } from "./projection.js";
import { caretDisplayOffset, measureTextOffset, measureTextRange } from "./visual_carets.js";

const RANGE_ENCODER = new TextEncoder();

/** Order two positions into `{ start, end }`. */
export function orderedRange(anchor, head) {
  return positionOrder(anchor, head) <= 0
    ? { start: anchor, end: head }
    : { start: head, end: anchor };
}

/** Document order of two source positions. */
export function positionOrder(left, right) {
  return left.line - right.line || left.byteInLine - right.byteInLine;
}

/**
 * Merged per-row rectangles covering `ordered`, clipped to the lines currently
 * carrying realized DOM. Lines outside the realized window have no geometry to
 * measure, which is also why they are not visible.
 *
 * `shouldIncludeLineBreak` adds the small stub that marks a consumed newline;
 * a selection draws it, a decoration does not.
 */
export function computeRangeRects(projection, ordered, shouldIncludeLineBreak) {
  const detailedRange = projectionDetailedRange(projection);
  const firstLine = Math.max(ordered.start.line, detailedRange.start);
  const lastLine = Math.min(ordered.end.line, detailedRange.end - 1);
  const rects = [];
  for (let lineIndex = firstLine; lineIndex <= lastLine; lineIndex += 1) {
    const layout = projectionLineLayout(projection, lineIndex);
    if (!layout) continue;
    const lineBytes = RANGE_ENCODER.encode(layout.sourceText).byteLength;
    const startByte = lineIndex === ordered.start.line ? ordered.start.byteInLine : 0;
    const endByte = lineIndex === ordered.end.line ? ordered.end.byteInLine : lineBytes;
    const start = caretDisplayOffset(layout, startByte);
    const end = caretDisplayOffset(layout, endByte);
    mergeRowBounds(measureTextRange(layout.element, start, end)).forEach((row) => rects.push(row));
    if (shouldIncludeLineBreak && lineIndex < ordered.end.line) {
      const point = measureTextOffset(layout.element, caretDisplayOffset(layout, lineBytes));
      rects.push({
        left: point.left,
        top: point.top,
        width: Math.max(2, point.height * 0.22),
        height: point.height,
      });
    }
  }
  return rects;
}

/** Position one absolutely placed overlay rectangle inside its own layer. */
export function placeOverlayRect(element, bounds, containerBounds) {
  element.style.transform = `translate(${bounds.left - containerBounds.left}px, ${bounds.top - containerBounds.top}px)`;
  element.style.width = `${Math.max(1, bounds.width)}px`;
  element.style.height = `${bounds.height}px`;
}

function mergeRowBounds(bounds) {
  const ordered = [...bounds].sort((left, right) => left.top - right.top || left.left - right.left);
  const rows = [];
  for (const boundsItem of ordered) {
    const previous = rows.at(-1);
    if (previous
        && Math.abs(previous.top - boundsItem.top) <= 1
        && Math.abs(previous.height - boundsItem.height) <= 1
        && boundsItem.left <= previous.left + previous.width + 1) {
      previous.width = Math.max(previous.width, boundsItem.right - previous.left);
    } else {
      rows.push({
        left: boundsItem.left,
        top: boundsItem.top,
        width: boundsItem.width,
        height: boundsItem.height,
      });
    }
  }
  return rows;
}
