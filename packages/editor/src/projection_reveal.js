// Host navigation measured against the rendered projection.
//
// Source positions do not have stable pixel geometry until the projection has
// rendered: headings scale, marker folding changes wrapping, and hanging
// indents move continuation rows. This module measures that final DOM and
// converts the requested visual movement back into the active scroller's
// coordinate space, including desktop projection-scroll compensation.

import { orderedRange } from "./overlay_rects.js";
import { projectionLineLayout } from "./projection.js";
import {
  projectionScrollOffset,
  scrollTopForProjectionOffset,
} from "./scroll_extent.js";
import { caretDisplayOffset, measureTextOffset, measureTextRange } from "./visual_carets.js";

/** Reveal one rendered source range inside the supplied scroll owner. */
export function revealProjectedRange(
  projection, scroller, requestedRange, align = "nearest", measuredRects = [],
) {
  const bounds = measuredRects.length > 0
    ? boundsFromRects(measuredRects)
    : measureProjectedRange(projection, requestedRange);
  if (!bounds || scroller.clientHeight === 0) return null;
  const viewport = scroller.getBoundingClientRect();
  const delta = align === "center"
    ? (bounds.top + bounds.bottom - viewport.top - viewport.bottom) / 2
    : computeNearestDelta(bounds, viewport);
  let shouldCorrect = false;
  let scrollDelta = 0;
  if (Math.abs(delta) > 0.5) {
    const targetOffset = projectionScrollOffset(scroller) + delta;
    const nextScrollTop = scrollTopForProjectionOffset(scroller, targetOffset);
    const isFullyVisible = bounds.top >= viewport.top && bounds.bottom <= viewport.bottom;
    scrollDelta = nextScrollTop - scroller.scrollTop;
    shouldCorrect = Math.abs(scrollDelta) > 0.5 || !isFullyVisible;
    scroller.scrollTop = nextScrollTop;
  }
  return { scrollDelta, shouldCorrect };
}

function measureProjectedRange(projection, requestedRange) {
  const range = orderedRange(requestedRange.start, requestedRange.end);
  if (range.start.line === range.end.line) {
    const layout = projectionLineLayout(projection, range.start.line);
    if (!layout) return null;
    const startOffset = caretDisplayOffset(layout, range.start.byteInLine);
    if (range.start.byteInLine === range.end.byteInLine) {
      return boundsFromRects([measureTextOffset(layout.element, startOffset)]);
    }
    const endOffset = caretDisplayOffset(layout, range.end.byteInLine);
    const rects = measureTextRange(layout.element, startOffset, endOffset);
    if (rects.length > 0) return boundsFromRects(rects);
  }

  const start = measurePosition(projection, range.start);
  const end = measurePosition(projection, range.end);
  return start && end ? boundsFromRects([start, end]) : null;
}

function measurePosition(projection, position) {
  const layout = projectionLineLayout(projection, position.line);
  if (!layout) return null;
  return measureTextOffset(layout.element, caretDisplayOffset(layout, position.byteInLine));
}

function boundsFromRects(rects) {
  if (rects.length === 0) return null;
  const top = Math.min(...rects.map((rect) => rect.top));
  const bottom = Math.max(...rects.map((rect) => rect.top + rect.height));
  return {
    top,
    bottom,
    height: bottom - top,
    rowHeight: Math.max(...rects.map((rect) => rect.height)),
  };
}

function computeNearestDelta(target, viewport) {
  if (target.height > viewport.height) {
    if (target.top > viewport.top) return target.top - viewport.top;
    if (target.bottom < viewport.bottom) return target.bottom - viewport.bottom;
    return 0;
  }
  const margin = Math.min(
    target.rowHeight,
    Math.max(0, (viewport.height - target.height) / 2),
  );
  if (target.top < viewport.top + margin) return target.top - viewport.top - margin;
  if (target.bottom > viewport.bottom - margin) return target.bottom - viewport.bottom + margin;
  return 0;
}
