import { utf8ByteToUtf16 } from "./coordinates.js";

const LIST_PREFIX = /^[\t ]*(?:[•☐☑]|[-*+]|\d+[.)])[\t ]+/u;
const WRAP_ENCODER = new TextEncoder();
const DEFAULT_TAB_COLUMNS = 4;
let measureContext;
let measureFont;

/** Find the source-visible prefix that wrapped rows hang beneath. */
export function computeSourceWrapPrefix(text) {
  const listPrefix = text.match(LIST_PREFIX)?.[0];
  const prefix = listPrefix ?? text.match(/^[\t ]*/u)?.[0] ?? "";
  return {
    byteEnd: WRAP_ENCODER.encode(prefix).length,
    isListItem: Boolean(listPrefix),
  };
}

/**
 * Font metrics one render pass measures against, read once from the projection
 * rather than per line.
 *
 * Lines outside the realized window used to fall back to `ch` units with a
 * hardcoded four-column tab. `ch` is the advance of `0`, which in a
 * proportional font is neither the space advance nor the tab stop, so an
 * unrealized line hung at a different offset than the same line one scroll
 * later. Measuring the container once costs a single style read per pass and
 * makes the two paths agree.
 */
export function computeWrapMetrics(container) {
  const style = getComputedStyle(container);
  const font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const columns = parseInt(style.tabSize, 10) || DEFAULT_TAB_COLUMNS;
  const context = ensureMeasureContext(font);
  const spaceAdvance = context ? context.measureText(" ").width : 0;
  return { font, spaceAdvance, tabWidth: spaceAdvance * columns };
}

/** Apply hanging indent, measuring exact pixels only for realized viewport lines. */
export function applyMeasuredWrapLayout(element, text, prefix, shouldMeasure, metrics) {
  if (prefix.byteEnd === 0) {
    if (element.dataset.wrapLayoutFingerprint !== "none") {
      element.style.setProperty("--continuity-wrap-indent", "0px");
      element.dataset.listItem = "false";
      element.dataset.breakUnbroken = "false";
      element.dataset.wrapLayoutFingerprint = "none";
    }
    return;
  }
  const prefixUtf16End = utf8ByteToUtf16(text, prefix.byteEnd);
  const prefixText = text.slice(0, prefixUtf16End);
  if (!shouldMeasure) {
    const approximate = metrics ?? computeWrapMetrics(element);
    const layoutFingerprint = `approximate\u0000${prefixText}\u0000${approximate.font}`;
    if (element.dataset.wrapLayoutFingerprint === layoutFingerprint) return;
    const token = text.slice(prefixUtf16End).match(/^[^\s]+/u)?.[0] ?? "";
    const prefixWidth = measureTextAdvance(prefixText, approximate);
    element.style.setProperty("--continuity-wrap-indent", `${prefixWidth}px`);
    element.dataset.listItem = String(prefix.isListItem);
    element.dataset.breakUnbroken = String(prefix.isListItem && token.length > 64);
    element.dataset.wrapLayoutFingerprint = layoutFingerprint;
    return;
  }
  const style = getComputedStyle(element);
  const layoutFingerprint = `${prefixText}\u0000${style.font}\u0000${element.clientWidth}`;
  if (element.dataset.wrapLayoutFingerprint === layoutFingerprint) return;
  const measured = elementWrapMetrics(style);
  const prefixWidth = measureTextAdvance(prefixText, measured);
  element.style.setProperty("--continuity-wrap-indent", `${prefixWidth}px`);
  element.dataset.listItem = String(prefix.isListItem);
  element.dataset.breakUnbroken = String(
    prefix.isListItem && hasOverwideFirstToken(element, text, prefixUtf16End, prefixWidth, measured),
  );
  element.dataset.wrapLayoutFingerprint = layoutFingerprint;
}

/**
 * Advance of `text` from the start of a rendered row, expanding tabs against
 * their own stop grid.
 *
 * The grid origin is the row's own start, which is what the styles guarantee:
 * the hanging indent is `text-indent: <width> hanging`, so the line box carries
 * no inline padding and every row — first and continuation alike — begins at
 * the projection's content edge. Expressing the same indent as
 * `padding-inline-start` plus a negative `text-indent` does not: CSS anchors tab
 * stops at the *content* edge, so the padding shifts the whole grid right by the
 * indent while the negative indent pulls the first row left by it, and a leading
 * tab then lands at `indent mod tab-width` instead of at the first stop. A
 * nested bullet's own content therefore drew left of the wrapped rows hanging
 * beneath it, by an amount that changed with the font. Space-indented lines were
 * unaffected, which is why only nested lines looked wrong.
 */
export function measureTextAdvance(text, metrics) {
  if (text.length === 0) return 0;
  const context = ensureMeasureContext(metrics.font);
  if (!context) return 0;
  const tabWidth = metrics.tabWidth > 0 ? metrics.tabWidth : 1;
  let width = 0;
  let runStart = 0;
  for (let index = text.indexOf("\t"); index >= 0; index = text.indexOf("\t", runStart)) {
    width += context.measureText(text.slice(runStart, index)).width;
    width = Math.floor(width / tabWidth + 1) * tabWidth;
    runStart = index + 1;
  }
  return width + context.measureText(text.slice(runStart)).width;
}

/** Metrics for one already-realized line, which may carry its own font size. */
export function elementWrapMetrics(style) {
  const font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const columns = parseInt(style.tabSize, 10) || DEFAULT_TAB_COLUMNS;
  const context = ensureMeasureContext(font);
  const spaceAdvance = context ? context.measureText(" ").width : 0;
  return { font, spaceAdvance, tabWidth: spaceAdvance * columns };
}

function hasOverwideFirstToken(element, text, contentStart, prefixWidth, metrics) {
  const token = text.slice(contentStart).match(/^[^\s]+/u)?.[0] ?? "";
  return token.length > 0
    && measureTextAdvance(token, metrics) > Math.max(1, element.clientWidth - prefixWidth);
}

function ensureMeasureContext(font) {
  measureContext ??= document.createElement("canvas").getContext("2d");
  if (!measureContext) return null;
  // The canvas normalizes whatever it is handed, so the requested string is
  // tracked separately rather than compared against `context.font`.
  if (measureFont !== font) {
    measureContext.font = font;
    measureFont = font;
  }
  return measureContext;
}
