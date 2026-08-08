// Which element owns scrolling, and how the caret is revealed within it.
//
// On a fine pointer the textarea scrolls itself, exactly as it always has. On a
// coarse pointer the finger must never land on the textarea — the platform's
// long-press selection is unrefusable on an editable element — so a shield div
// covers it, takes the touch, and scrolls instead. Its spacer is sized to the
// projection, which makes the scrollable extent equal to the height of what the
// reader can see rather than to the height of a hidden textarea whose layout
// differs from it.

let touchPointerQuery;

/** Whether this device's primary pointer routes through the touch shield. */
export function isTouchScrolling() {
  if (typeof matchMedia !== "function") return false;
  touchPointerQuery ??= matchMedia("(pointer: coarse)");
  return touchPointerQuery.matches;
}

/** Subscribe to primary-pointer changes so the surface can be re-resolved. */
export function observeTouchPointer(onChange) {
  if (typeof matchMedia !== "function") return () => {};
  touchPointerQuery ??= matchMedia("(pointer: coarse)");
  touchPointerQuery.addEventListener("change", onChange);
  return () => touchPointerQuery.removeEventListener("change", onChange);
}

/**
 * Size the shield's spacer to the projection so the scrollable range covers
 * every rendered row, and mirror the shield's offset onto the textarea so the
 * legacy measurement paths keep seeing a coherent viewport.
 */
export function synchronizeShieldExtent(surface) {
  const { shield, shieldSpacer, projection } = surface;
  if (!shield || !shieldSpacer) return;
  // The projection's height, not the textarea's. The projection renders every
  // source line, so it covers the whole document; the textarea's height is a
  // different function of the same text and is no longer visible or scrolled.
  // Taking the larger of the two would add dead scroll past the last row.
  const next = `${projection.offsetHeight}px`;
  if (shieldSpacer.style.height !== next) shieldSpacer.style.height = next;
}

/** The element whose scroll offset drives the projection. */
export function activeScroller(surface) {
  return isTouchScrolling() && surface.shield ? surface.shield : surface.input;
}
