// The textarea owns the scrollable extent, but the projection is what the
// reader sees, and the two are different functions of the same source. A
// projected heading renders at up to 1.45em inside a box 14px narrower than the
// textarea, so one source line can occupy more visual rows in the projection
// than it does in the textarea. The surplus is unreachable: `.frame` clips with
// `overflow: hidden` and the projection is only translated by `-scrollTop`, so
// once the textarea hits its own maximum the projection tail stays below the
// fold forever.
//
// Two levers keep the scroll ranges aligned, in order of preference:
//
//  1. Bottom padding on the textarea, which lengthens its scrollable content
//     and keeps the projection tracking the text 1:1. This is bounded: the
//     textarea is `inset: 0` and `box-sizing: border-box`, so padding taller
//     than the frame inflates the element's own border box, growing
//     `clientHeight` and cancelling the extra extent it just bought.
//  2. A proportional offset applied to the projection transform for whatever
//     range difference padding cannot absorb. It ramps from zero at the top to
//     the signed residual at the scroll floor, so a projection that is either
//     taller or shorter than the textarea reaches its own tail without blank
//     overscroll.

const EXTENT_EPSILON = 1;
// `getComputedStyle` and the extent arithmetic sit on the render path, so the
// base padding is read once per element and the whole pass is skipped whenever
// neither the projection nor the textarea content has changed size.
const extentStates = new WeakMap();

/**
 * Grow the textarea's scrollable extent to cover a taller projection.
 * Returns the residual the transform must absorb.
 */
export function synchronizeScrollExtent(input, projection) {
  // A hidden editor (display:none tab switch) measures zero; keep whatever was
  // computed while it was visible rather than collapsing it.
  if (input.clientWidth === 0 && input.clientHeight === 0) return readResidual(input);
  let state = extentStates.get(input);
  if (state === undefined) {
    state = { basePadding: Number.parseFloat(getComputedStyle(input).paddingTop) || 0 };
    extentStates.set(input, state);
  }
  const { basePadding } = state;
  const applied = readAppliedPadding(input);
  // `scrollHeight` already includes applied padding; recover the intrinsic
  // content height before comparing against the projection.
  const scrollHeight = input.scrollHeight;
  const projectionHeight = projection.offsetHeight;
  // The textarea's own client height, not the frame's: the command rail insets
  // the textarea's bottom, so the frame overstates how much of it the reader can
  // see. Capping below the inflation threshold keeps this measurement honest —
  // an uninflated textarea always reports its natural height here.
  const visibleHeight = input.clientHeight;
  if (state.scrollHeight === scrollHeight
      && state.projectionHeight === projectionHeight
      && state.visibleHeight === visibleHeight) {
    return readResidual(input);
  }
  Object.assign(state, { scrollHeight, projectionHeight, visibleHeight });
  const intrinsicHeight = scrollHeight - applied;
  const deficit = Math.max(0, Math.ceil(projectionHeight - intrinsicHeight));
  // Both the base top padding and the base bottom padding count against the
  // border box, so the surplus has to leave room for each. Subtracting only one
  // let the textarea inflate past the frame, and every pixel it grew by was a
  // pixel of scroll range lost again.
  const maximumPadding = Math.max(0, visibleHeight - basePadding * 2);
  const padding = Math.min(deficit, maximumPadding);
  if (Math.abs(padding - applied) > EXTENT_EPSILON) writeAppliedPadding(input, padding, basePadding);
  const residual = projectionHeight - intrinsicHeight - padding;
  writeResidual(input, residual);
  return residual;
}

/** Extra downward shift the projection needs at the current scroll offset. */
export function projectionScrollCompensation(input) {
  const residual = readResidual(input);
  if (residual === 0) return 0;
  const range = input.scrollHeight - input.clientHeight;
  if (range <= 0) return 0;
  // Ramp linearly so the top stays aligned and the floor reveals the tail.
  return residual * Math.min(1, Math.max(0, input.scrollTop / range));
}

/** Projection-space offset represented by the current scroll owner offset. */
export function projectionScrollOffset(input) {
  return input.scrollTop + projectionScrollCompensation(input);
}

/** Scroll-owner offset that represents one requested projection-space offset. */
export function scrollTopForProjectionOffset(input, requestedOffset) {
  const range = Math.max(0, input.scrollHeight - input.clientHeight);
  if (range === 0) return 0;
  const projectionRange = range + readResidual(input);
  if (projectionRange <= 0) return 0;
  const scale = projectionRange / range;
  return Math.min(range, Math.max(0, requestedOffset / scale));
}

function readAppliedPadding(input) {
  const raw = Number.parseFloat(input.dataset.scrollExtentPadding ?? "");
  return Number.isFinite(raw) ? raw : 0;
}

function writeAppliedPadding(input, pixels, basePadding) {
  if (pixels === 0) {
    input.style.removeProperty("padding-block-end");
    delete input.dataset.scrollExtentPadding;
    return;
  }
  input.style.setProperty("padding-block-end", `${basePadding + pixels}px`);
  input.dataset.scrollExtentPadding = String(pixels);
}

function readResidual(input) {
  const raw = Number.parseFloat(input.dataset.scrollExtentResidual ?? "");
  return Number.isFinite(raw) ? raw : 0;
}

function writeResidual(input, residual) {
  if (residual === 0) delete input.dataset.scrollExtentResidual;
  else input.dataset.scrollExtentResidual = String(residual);
}
