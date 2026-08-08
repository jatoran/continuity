// Indent guides for the DOM projection.
//
// The native Win32 chrome paints these directly (`chrome_indent_guides.rs`);
// the browser projection has no chrome layer, so each guide column is painted
// as a hard-stop gradient in the line's own background. Backgrounds resolve
// against the padding box, which no per-line inline padding can move, so a
// guide column keeps its x across lines with different hanging indents.
//
// The column semantics mirror the desktop painter exactly: a guide at offset C
// means "an enclosing parent's content starts at C", the body-left-edge column
// is suppressed, a blank line inherits the columns its two non-blank
// neighbours share, and the caret's line draws its deepest column in the
// active colour.

/** How far a blank line looks for a non-blank neighbour to inherit from. */
const BLANK_LINE_SKIRT = 64;

/** Whether the host asked this projection to paint indent guides. */
export function hasIndentGuides(container) {
  return container.dataset.indentGuides === "on";
}

/**
 * Paint one line's guide columns into `--continuity-line-guides`, or clear it.
 * The stylesheet only consumes the property while guides are enabled, so a
 * value left behind by a disabled pass paints nothing.
 */
export function applyIndentGuides(element, context, index, metrics) {
  const guides = computeLineIndentGuides(context, index, metrics);
  const isActive = context.activeLines?.has(index) === true;
  const image = guides ? guideImage(guides.bounds, guides.depth, isActive) : "";
  const fingerprint = `${image}`;
  if (element.dataset.indentGuideFingerprint === fingerprint) return;
  element.dataset.indentGuideFingerprint = fingerprint;
  if (image) {
    element.style.setProperty("--continuity-line-guides", image);
  } else {
    element.style.removeProperty("--continuity-line-guides");
  }
}

/** Drop every cached guide column; the next pass re-measures. */
export function clearIndentGuideCache(context) {
  context.indentGuideCache = undefined;
}

/**
 * Guide columns for one source line: its own when it carries text, otherwise
 * the columns both surrounding non-blank lines agree on.
 */
function computeLineIndentGuides(context, index, metrics) {
  const cache = ensureCache(context, metrics);
  const lines = context.sourceLines;
  const own = boundariesForLine(lines, index, metrics, cache);
  if (own) return { bounds: own, depth: own.length };
  const previous = scanNonBlank(lines, index, -1, metrics, cache);
  const following = scanNonBlank(lines, index, 1, metrics, cache);
  if (!previous || !following) return null;
  const depth = Math.min(previous.length, following.length);
  return depth > 0 ? { bounds: previous.slice(0, depth), depth } : null;
}

function scanNonBlank(lines, index, step, metrics, cache) {
  const limit = Math.min(BLANK_LINE_SKIRT, step < 0 ? index : lines.length - index);
  for (let distance = 1; distance <= limit; distance += 1) {
    const bounds = boundariesForLine(lines, index + step * distance, metrics, cache);
    if (bounds) return bounds;
  }
  return null;
}

function boundariesForLine(lines, index, metrics, cache) {
  const text = lines[index];
  if (text === undefined) return null;
  const leading = text.match(/^[\t ]*/u)?.[0] ?? "";
  if (leading.length === text.length) return null;
  const cached = cache.get(leading);
  if (cached !== undefined) return cached;
  const bounds = computeIndentBoundaries(leading, metrics);
  cache.set(leading, bounds);
  return bounds;
}

/**
 * Where each indent unit on a line ends, in pixels from the text origin. A tab
 * runs to its own rendered stop; a run of `columns` spaces is one unit. A
 * trailing partial space run does not form a level, matching the desktop
 * painter — a half-indent is not a parent.
 */
export function computeIndentBoundaries(leading, metrics) {
  const columns = Math.max(1, Math.round(metrics.tabWidth / Math.max(metrics.spaceAdvance, 0.01)));
  const bounds = [];
  let width = 0;
  let spaceRun = 0;
  for (const character of leading) {
    if (character === "\t") {
      // Mixed `   \t` indentation collapses onto the tab's own stop, which is
      // where the glyphs after it actually render.
      width -= spaceRun * metrics.spaceAdvance;
      spaceRun = 0;
      width = Math.floor(width / Math.max(metrics.tabWidth, 0.01) + 1) * metrics.tabWidth;
      bounds.push(width);
      continue;
    }
    width += metrics.spaceAdvance;
    spaceRun += 1;
    if (spaceRun >= columns) {
      bounds.push(width);
      spaceRun = 0;
    }
  }
  return bounds;
}

function guideImage(bounds, depth, isActive) {
  const stops = [];
  let previous = 0;
  for (let level = 1; level < depth; level += 1) {
    // Whole pixels: a 1px rule on a fractional offset resolves to two
    // half-covered device columns and reads as a smudge rather than a line.
    const left = Math.round(bounds[level - 1]);
    if (left < previous) continue;
    const color = isActive && level === depth - 1
      ? "var(--continuity-indent-guide-active)"
      : "var(--continuity-indent-guide)";
    stops.push(`transparent ${previous}px ${left}px`, `${color} ${left}px ${left + 1}px`);
    previous = left + 1;
  }
  if (stops.length === 0) return "";
  stops.push(`transparent ${previous}px`);
  return `linear-gradient(to right, ${stops.join(", ")})`;
}

function ensureCache(context, metrics) {
  if (!context.indentGuideCache || context.indentGuideFont !== metrics.font) {
    context.indentGuideCache = new Map();
    context.indentGuideFont = metrics.font;
  }
  return context.indentGuideCache;
}
