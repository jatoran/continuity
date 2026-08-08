// What the reader can currently see, and what happens when it changes.
//
// Two halves of one concept: the scroll handler re-realizes the projection
// around a new offset, and the watch publishes which source lines that offset
// actually shows.
//
// The visible window is measured, never derived. A host cannot compute it from
// `getScrollState`, because pixels do not convert to lines outside this module:
// a projected heading renders at up to 1.45em, wrapped rows carry a measured
// pixel hanging indent, unrealized lines are laid out against the projection's
// own font metrics, and either the textarea or the touch shield may be the
// scroller depending on a live `pointer: coarse` media query. There is no line
// height to divide by.
//
// Client rectangles are used rather than `offsetTop` for the same reason. They
// already carry the scroll transform, the extent compensation ramp, and
// whichever surface currently scrolls, so none of that has to be re-derived
// here and kept in step with `scroll_extent.js` and `scroll_surface.js`. The
// projection holds exactly one element per source line in document order, so a
// binary search over those rectangles answers the question directly - and a
// wrapped line resolves to its own source line for free, because its extra
// visual rows live inside that one element.
//
// This is the *visible* window, not the realized one. The projection realizes
// two further viewports in each direction so scrolling has DOM ready ahead of
// it; publishing that window would name a line two screens from the reader.

import { renderProjectionViewport } from "./projection.js";

/** Re-realize the projection and repaint chrome after the surface scrolls. */
export function handleEditorScroll(ctx) {
  // The selection action bar follows rather than dismissing: it is anchored to
  // the painted highlight, which moves with the projection.
  ctx.updateSelectionActions?.();
  renderProjectionViewport(ctx.projection, ctx.scroller());
  ctx.syncScroll();
  // Scrolling may re-realize the composing line from stale engine text and
  // repaint stale engine overlays; the live preview repaint wins the turn.
  if (ctx.isComposing()) ctx.renderComposing();
  ctx.probeViewport();
}

/**
 * First and last source lines with any pixel inside the scroller's viewport.
 *
 * Partial lines count at both edges: a heading straddling the top edge is
 * still the section the reader is in, and reporting the first *fully* visible
 * line would make a host's sticky chrome flicker as that heading scrolls out.
 *
 * Returns `null` when there is nothing to measure - before the first layout,
 * or while the host keeps the editor in a `display: none` tab. A hidden
 * element measures zero everywhere, which would otherwise be indistinguishable
 * from "line 0 fills the viewport".
 */
export function computeVisibleLineRange(projection, scroller) {
  const children = projection?.children;
  if (!children || children.length === 0 || !scroller) return null;
  const viewport = scroller.getBoundingClientRect();
  if (viewport.height === 0) return null;
  const lastIndex = children.length - 1;
  const startLine = Math.min(firstLineEndingAfter(children, viewport.top), lastIndex);
  const endLine = Math.max(startLine, lastLineStartingBefore(children, viewport.bottom));
  return { startLine, endLine };
}

/**
 * Publish the visible window whenever it changes.
 *
 * Measurement is deferred to an animation frame and coalesced to one per
 * frame: a scroll fires continuously, and reading a client rectangle forces
 * layout, so probing per event would both stall the scroll and hand a host
 * more repaints than it can use. Identical consecutive windows are dropped
 * because the event exists to drive a host repaint, and scrolling within one
 * line is not news.
 */
export function createViewportWatch(ctx, emit) {
  let frame;
  let published = null;
  const read = () => computeVisibleLineRange(ctx.projection, ctx.scroller());
  const publish = (range) => {
    published = range;
    emit(range);
  };
  return {
    read,
    /** Measure on the next frame; repeated calls within one frame collapse. */
    schedule() {
      if (frame !== undefined) return;
      frame = requestAnimationFrame(() => {
        frame = undefined;
        const range = read();
        if (!range) return;
        if (published?.startLine === range.startLine && published?.endLine === range.endLine) return;
        publish(range);
      });
    },
    /** Seed the host's initial value; publishes even when nothing changed. */
    publishNow() {
      const range = read();
      if (range) publish(range);
    },
    destroy() {
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = undefined;
    },
  };
}

/** First line index with any pixel past `edge`, or `children.length` if none. */
function firstLineEndingAfter(children, edge) {
  let lower = 0;
  let upper = children.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (children[middle].getBoundingClientRect().bottom <= edge) lower = middle + 1;
    else upper = middle;
  }
  return lower;
}

/** Last line index that starts before `edge`, or `-1` if none does. */
function lastLineStartingBefore(children, edge) {
  let lower = 0;
  let upper = children.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (children[middle].getBoundingClientRect().top < edge) lower = middle + 1;
    else upper = middle;
  }
  return lower - 1;
}
