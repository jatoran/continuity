import { measureTextOffset, measureTextRange } from "./visual_carets.js";
import { applyMeasuredWrapLayout, computeSourceWrapPrefix } from "./wrap_layout.js";
import { clearProjectionCompositionLine, setProjectionCompositionLine } from "./projection.js";

// Distinct from every fingerprint renderProjectionLine can compute, so the
// commit render always replaces preview DOM (dropping the composed-run marker).
const PREVIEW_FINGERPRINT_PREFIX = "composition-preview\u0000";

/**
 * Paint the line-scoped IME composition presentation. The Markdown projection
 * stays visible for the whole document; only the composing source line
 * previews the live textarea text (a DOM-only update — the engine is never
 * written mid-composition), and the primary caret plus any same-line selection
 * are painted from the live textarea selection against the previewed glyphs.
 * Mobile keyboards compose continuously, so anything frame-wide here would
 * flash the entire document between raw source and rendered Markdown on every
 * word. When the live text cannot be mapped onto the projected line structure
 * (for example a composed run containing a newline), the frame falls back to
 * the whole-frame native reveal so the writer is never typing blind.
 */
export function renderCompositionPresentation(frame, projection, carets, input, composedRunLength) {
  const isPreviewLive = renderCompositionPreviewLine(projection, input, composedRunLength);
  frame.classList.toggle("composing-fallback", !isPreviewLive);
  if (isPreviewLive) {
    renderCompositionOverlay(carets, projection, input);
  } else {
    carets.replaceChildren();
  }
}

/** Remove composition-only presentation state once the composition commits. */
export function clearCompositionPresentation(frame) {
  frame.classList.remove("composing-fallback");
}

function renderCompositionPreviewLine(projection, input, composedRunLength) {
  const bounds = composingLineBounds(input);
  if (countLines(input.value) !== projection.children.length) {
    clearProjectionCompositionLine(projection);
    return false;
  }
  const element = projection.children[bounds.lineIndex];
  if (!element) {
    clearProjectionCompositionLine(projection);
    return false;
  }
  const text = input.value.slice(bounds.lineStart, bounds.lineEnd);
  const fingerprint = `${PREVIEW_FINGERPRINT_PREFIX}${composedRunLength}\u0000${input.selectionEnd}\u0000${text}`;
  if (element.dataset.fingerprint !== fingerprint) {
    renderPreviewContent(element, input, bounds, text, composedRunLength);
    element.dataset.fingerprint = fingerprint;
  }
  if (element.dataset.sourceVisible !== "true") {
    element.dataset.sourceVisible = "true";
  }
  applyMeasuredWrapLayout(element, text, computeSourceWrapPrefix(text), true);
  // A tap during composition must hit-test against this live line, not the
  // still-pre-composition engine mirror.
  setProjectionCompositionLine(projection, bounds.lineIndex, text);
  return true;
}

function renderPreviewContent(element, input, bounds, text, composedRunLength) {
  const caretInLine = clampToLine(input.selectionEnd ?? 0, bounds) - bounds.lineStart;
  const runStart = Math.max(0, caretInLine - composedRunLength);
  const fragment = document.createDocumentFragment();
  if (composedRunLength > 0 && runStart < caretInLine) {
    fragment.append(document.createTextNode(text.slice(0, runStart)));
    const run = document.createElement("span");
    run.className = "composition-run";
    run.textContent = text.slice(runStart, caretInLine);
    fragment.append(run, document.createTextNode(text.slice(caretInLine)));
  } else {
    fragment.append(document.createTextNode(text));
  }
  if (text.length === 0) {
    fragment.append(document.createElement("br"));
  }
  element.replaceChildren(fragment);
}

/** Paint the live textarea caret and same-line selection against the preview. */
function renderCompositionOverlay(carets, projection, input) {
  const bounds = composingLineBounds(input);
  const element = projection.children[bounds.lineIndex];
  const start = input.selectionStart ?? 0;
  const end = input.selectionEnd ?? start;
  if (!element || end > bounds.lineEnd) {
    carets.replaceChildren();
    return;
  }
  const containerBounds = carets.getBoundingClientRect();
  const fragment = document.createDocumentFragment();
  for (const row of measureTextRange(element, start - bounds.lineStart, end - bounds.lineStart)) {
    const highlight = document.createElement("span");
    highlight.className = "visual-selection";
    highlight.style.transform = `translate(${row.left - containerBounds.left}px, ${row.top - containerBounds.top}px)`;
    highlight.style.width = `${Math.max(1, row.width)}px`;
    highlight.style.height = `${row.height}px`;
    fragment.append(highlight);
  }
  const head = input.selectionDirection === "backward" ? start : end;
  const point = measureTextOffset(element, head - bounds.lineStart);
  const caret = document.createElement("span");
  caret.className = "visual-caret primary-caret";
  caret.style.transform = `translate(${point.left - containerBounds.left}px, ${point.top - containerBounds.top}px)`;
  caret.style.height = `${point.height}px`;
  fragment.append(caret);
  carets.replaceChildren(fragment);
}

/** Locate the composing source line and its bounds in the live textarea value. */
function composingLineBounds(input) {
  const text = input.value;
  const caret = input.selectionStart ?? 0;
  const lineStart = caret === 0 ? 0 : text.lastIndexOf("\n", caret - 1) + 1;
  const newlineAfterCaret = text.indexOf("\n", caret);
  return {
    lineIndex: countLines(text.slice(0, lineStart)) - 1,
    lineStart,
    lineEnd: newlineAfterCaret < 0 ? text.length : newlineAfterCaret,
  };
}

function clampToLine(offset, bounds) {
  return Math.max(bounds.lineStart, Math.min(offset, bounds.lineEnd));
}

function countLines(text) {
  let count = 1;
  for (let index = text.indexOf("\n"); index >= 0; index = text.indexOf("\n", index + 1)) {
    count += 1;
  }
  return count;
}
