import {
  sourceLineStarts,
  utf8ByteToUtf16,
} from "./coordinates.js";
import { applySourceEdits } from "./source_lines.js";
import {
  applyMeasuredWrapLayout,
  computeSourceWrapPrefix,
  computeWrapMetrics,
} from "./wrap_layout.js";
import { applyIndentGuides, hasIndentGuides } from "./indent_guides.js";
import { projectionPointToPosition } from "./pointer_hit_test.js";
import { projectionScrollOffset } from "./scroll_extent.js";

const INLINE_PRIORITY = ["link", "checkbox", "code", "strong", "emphasis", "strikethrough"];
const STRUCTURAL_MARKER_CHARS = /[\n#*_~`>[\]()-]/u;
const PROJECTION_ENCODER = new TextEncoder();
const PROJECTION_OVERSCAN_VIEWPORTS = 2;
const projectionStates = new WeakMap();

/** Build an identity projection without invoking the Markdown parser. */
export function createPlainPresentation(text) {
  let sourceByte = 0;
  const lines = text.split("\n").map((line) => {
    const byteLength = PROJECTION_ENCODER.encode(line).length;
    const report = {
      text: line,
      wrapIndentByteEnd: PROJECTION_ENCODER.encode(line.match(/^[\t ]*/u)?.[0] ?? "").length,
      sourceToDisplay: [],
      displayToSource: [],
      segments: [{
        kind: "visible",
        startByte: sourceByte,
        endByte: sourceByte + byteLength,
        replacement: "",
      }],
    };
    sourceByte += byteLength + 1;
    return report;
  });
  return { blocks: [], inlines: [], lines };
}

export function renderProjection(container, input, snapshot, projection, activeLines) {
  const sourceLines = snapshot.text.split("\n");
  const lineStarts = sourceLineStarts(snapshot.text);
  const blockClasses = computeBlockClasses(projection.blocks, lineStarts);
  const lineInlines = bucketInlinesByLine(projection.inlines, lineStarts);
  ensureLineElements(container, projection.lines.length);
  // Carry the detailed window forward so every line reconciles once at its
  // final detail level; unchanged lines keep their fingerprint and their DOM.
  const previousRange = projectionStates.get(container)?.detailedRange ?? { start: 0, end: 0 };
  const detailedRange = {
    start: Math.min(previousRange.start, projection.lines.length),
    end: Math.min(previousRange.end, projection.lines.length),
  };
  const context = {
    activeLines,
    blockClasses,
    detailedRange,
    inlineDigests: lineInlines.map((spans, index) => computeInlineDigest(spans, lineStarts[index])),
    lineInlines,
    projectedLines: projection.lines.slice(),
    sourceLines,
    hasStructuralDirty: false,
  };
  projectionStates.set(container, context);
  refreshLayoutMetrics(container, context);
  projection.lines.forEach((_, index) => renderProjectionLine(
    container, context, index, index >= detailedRange.start && index < detailedRange.end,
  ));
  renderProjectionViewport(container, input);
}

/** Map a client-space click through the rendered projection into source coordinates. */
export function projectionPositionAtPoint(container, clientX, clientY) {
  const context = projectionStates.get(container);
  return context ? projectionPointToPosition(container, context, clientX, clientY) : null;
}

/**
 * Point one source line's hit-testing at the live IME-composition text. While a
 * composition previews on a source-visible line the engine still holds the
 * pre-composition text, so mapping a measured glyph offset through the stale
 * `sourceLines` mirror lands the caret on the wrong byte. The override supplies
 * the live textarea line instead; it is cleared once the composition settles.
 */
export function setProjectionCompositionLine(container, lineIndex, text) {
  const context = projectionStates.get(container);
  if (context) context.compositionLine = { index: lineIndex, text };
}

/** Drop the live-composition hit-test override once composition settles. */
export function clearProjectionCompositionLine(container) {
  const context = projectionStates.get(container);
  if (context) context.compositionLine = null;
}

/** Return the rendered state needed to place a visual caret on one source line. */
export function projectionLineLayout(container, lineIndex) {
  const context = projectionStates.get(container);
  const element = container.children[lineIndex];
  if (!context || !element) return null;
  return {
    element,
    line: context.projectedLines[lineIndex],
    sourceText: context.sourceLines[lineIndex] ?? "",
    isDetailed: element.dataset.detailed === "true",
    isSourceVisible: element.dataset.sourceVisible === "true",
  };
}

/** Return the source-line window currently carrying detailed projected DOM. */
export function projectionDetailedRange(container) {
  return projectionStates.get(container)?.detailedRange ?? { start: 0, end: 0 };
}

/** Return the source lines currently revealing raw text, if any are tracked. */
export function projectionActiveLines(container) {
  return projectionStates.get(container)?.activeLines ?? null;
}

/** Whether an idle pass can change visible projection rather than the active source line. */
export function shouldRefreshProjection(container, activeLines) {
  const context = projectionStates.get(container);
  if (!context) return true;
  if (context.hasStructuralDirty) return true;
  for (let index = context.detailedRange.start; index < context.detailedRange.end; index += 1) {
    if (!context.projectedLines[index] && !activeLines.has(index)) return true;
  }
  return false;
}

/** Whether Markdown structure, rather than only one active line, may have changed. */
export function isProjectionStructuralDirty(container) {
  return projectionStates.get(container)?.hasStructuralDirty === true;
}

/** Merge a viewport-scoped engine report into an existing projection. */
export function renderProjectionRange(container, input, snapshot, patch, activeLines) {
  const context = projectionStates.get(container);
  if (!context || patch.lineCount !== context.sourceLines.length) return false;
  const relativeBlocks = computeBlockClasses(patch.blocks, patch.lineStarts);
  const relativeInlines = bucketInlinesByLine(patch.inlines, patch.lineStarts);
  for (let offset = 0; offset < patch.lines.length; offset += 1) {
    const index = patch.startLine + offset;
    context.projectedLines[index] = patch.lines[offset];
    context.blockClasses[index] = relativeBlocks[offset] ?? "block-plain";
    context.lineInlines[index] = relativeInlines[offset] ?? [];
    context.inlineDigests[index] = computeInlineDigest(
      context.lineInlines[index], patch.lineStarts[offset] ?? 0,
    );
  }
  context.activeLines = activeLines;
  context.hasStructuralDirty = false;
  renderProjectionViewport(container, input, false);
  return true;
}

/** Realize projected DOM by measured pixel offsets around the browser viewport. */
export function renderProjectionViewport(container, input, shouldMeasure = true) {
  const context = projectionStates.get(container);
  if (!context || container.children.length === 0) {
    return;
  }
  refreshLayoutMetrics(container, context);
  if (!shouldMeasure) {
    applyProjectionViewportRange(container, context, context.detailedRange);
    return;
  }
  let nextRange = computeProjectionViewportRange(container, input);
  for (let pass = 0; pass < 2; pass += 1) {
    applyProjectionViewportRange(container, context, nextRange);
    const measuredRange = computeProjectionViewportRange(container, input);
    if (measuredRange.start === nextRange.start && measuredRange.end === nextRange.end) {
      break;
    }
    nextRange = measuredRange;
  }
}

function applyProjectionViewportRange(container, context, nextRange) {
  const previousRange = context.detailedRange;
  for (let index = previousRange.start; index < previousRange.end; index += 1) {
    if (index < nextRange.start || index >= nextRange.end) {
      renderProjectionLine(container, context, index, false);
    }
  }
  for (let index = nextRange.start; index < nextRange.end; index += 1) {
    renderProjectionLine(container, context, index, true);
  }
  context.detailedRange = nextRange;
}

/**
 * Re-read the font metrics unrealized lines and indent guides measure against.
 * Once per pass: a host restyle (font family, size, or `tab-size`) changes both
 * the hanging indent and the guide grid, and a stale metric would leave the two
 * disagreeing until the next full render.
 */
function refreshLayoutMetrics(container, context) {
  context.metrics = computeWrapMetrics(container);
  context.hasIndentGuides = hasIndentGuides(container);
}

function renderProjectionLine(container, context, index, shouldProject) {
  const element = container.children[index];
  const line = context.projectedLines[index];
  if (!element) {
    return;
  }
  const sourceText = context.sourceLines[index] ?? "";
  const canProject = shouldProject && Boolean(line);
  const isSourceVisible = context.activeLines.has(index);
  const text = isSourceVisible || !canProject ? sourceText : line.text;
  const className = line
    ? `line ${context.blockClasses[index] ?? "block-plain"}`
    : "line block-plain";
  const projectedWrapPrefix = line?.wrapIndentByteEnd ?? 0;
  const wrapPrefix = isSourceVisible || !canProject
    ? computeSourceWrapPrefix(text)
    : { byteEnd: projectedWrapPrefix, isListItem: projectedWrapPrefix > 0 && /^[\t ]*(?:[•☐☑]|[-*+]|\d+[.)])[\t ]+/u.test(text) };
  const inlineDigest = canProject && !isSourceVisible ? context.inlineDigests?.[index] ?? "" : "";
  const fingerprint = `${className}\u0000${isSourceVisible}\u0000${wrapPrefix.byteEnd}\u0000${text}\u0000${canProject}\u0000${inlineDigest}`;
  if (element.className !== className) element.className = className;
  if (element.dataset.line !== String(index)) element.dataset.line = String(index);
  if (element.dataset.sourceVisible !== String(isSourceVisible)) {
    element.dataset.sourceVisible = String(isSourceVisible);
  }
  if (element.dataset.fingerprint !== fingerprint) {
    const ranges = canProject && !isSourceVisible
      ? inlineRanges(line, context.lineInlines[index])
      : [];
    renderLine(element, text, ranges);
    element.dataset.detailed = String(Boolean(canProject));
    element.dataset.fingerprint = fingerprint;
  }
  applyMeasuredWrapLayout(element, text, wrapPrefix, canProject, context.metrics);
  if (context.hasIndentGuides) applyIndentGuides(element, context, index, context.metrics);
}

export function renderActiveSourceLines(container, snapshot, activeLines, edits = []) {
  const context = projectionStates.get(container);
  if (!context) return false;
  const previousActiveLines = context.activeLines;
  context.hasStructuralDirty ||= edits.some(
    (edit) => isStructurallySignificantEdit(edit, context.sourceLines),
  );
  const sourceLines = applySourceEdits(context.sourceLines, edits, snapshot.text);
  context.activeLines = activeLines;
  context.sourceLines = sourceLines;
  refreshLayoutMetrics(container, context);
  reconcileEditedLines(container, context, edits);
  if (container.children.length !== sourceLines.length) {
    return false;
  }
  const linesToRender = new Set([...previousActiveLines, ...activeLines]);
  edits.forEach((edit) => {
    const insertedLines = countNewlines(edit.insertedText);
    for (let offset = 0; offset <= insertedLines; offset += 1) {
      linesToRender.add(edit.startLine + offset);
    }
  });
  for (const lineIndex of linesToRender) {
    const shouldProject = lineIndex >= context.detailedRange.start
      && lineIndex < context.detailedRange.end;
    renderProjectionLine(container, context, lineIndex, shouldProject);
  }
  return true;
}

/** Whether an edit can change Markdown structure beyond its own source line. */
function isStructurallySignificantEdit(edit, sourceLines) {
  if (STRUCTURAL_MARKER_CHARS.test(edit.insertedText)) return true;
  if (edit.removedBytes === 0) return false;
  if (edit.startLine !== edit.endLine) return true;
  const line = sourceLines[edit.startLine];
  if (line === undefined
    || !Number.isInteger(edit.startUtf16InLine)
    || !Number.isInteger(edit.endUtf16InLine)) {
    return true;
  }
  return STRUCTURAL_MARKER_CHARS.test(line.slice(edit.startUtf16InLine, edit.endUtf16InLine));
}

function computeProjectionViewportRange(container, input) {
  const overscan = input.clientHeight * PROJECTION_OVERSCAN_VIEWPORTS;
  const scrollOffset = projectionScrollOffset(input);
  const visibleStart = Math.max(0, scrollOffset - overscan);
  const visibleEnd = scrollOffset + input.clientHeight + overscan;
  const children = container.children;
  return {
    start: lowerBoundLineEnd(children, visibleStart),
    end: lowerBoundLineStart(children, visibleEnd),
  };
}

function lowerBoundLineEnd(children, target) {
  let lower = 0;
  let upper = children.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    const line = children[middle];
    if (line.offsetTop + line.offsetHeight < target) lower = middle + 1;
    else upper = middle;
  }
  return lower;
}

function lowerBoundLineStart(children, target) {
  let lower = 0;
  let upper = children.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (children[middle].offsetTop <= target) lower = middle + 1;
    else upper = middle;
  }
  return lower;
}

function reconcileEditedLines(container, context, edits) {
  for (const edit of edits) {
    const removedLines = edit.endLine - edit.startLine;
    const insertedLines = countNewlines(edit.insertedText);
    invalidateEditedProjection(context, edit.startLine, removedLines, insertedLines);
    if (insertedLines > removedLines) {
      const fragment = document.createDocumentFragment();
      for (let index = removedLines; index < insertedLines; index += 1) {
        const line = document.createElement("div");
        line.className = "line";
        fragment.append(line);
      }
      container.insertBefore(fragment, container.children[edit.startLine + 1] ?? null);
    } else {
      for (let index = insertedLines; index < removedLines; index += 1) {
        container.children[edit.startLine + 1]?.remove();
      }
    }
  }
}

function invalidateEditedProjection(context, startLine, removedLines, insertedLines) {
  if (!context) {
    return;
  }
  const spliceAt = startLine + 1;
  context.projectedLines.splice(spliceAt, removedLines, ...Array(insertedLines).fill(null));
  context.blockClasses.splice(spliceAt, removedLines, ...Array(insertedLines).fill("block-plain"));
  context.lineInlines.splice(
    spliceAt,
    removedLines,
    ...Array.from({ length: insertedLines }, () => []),
  );
  context.inlineDigests.splice(spliceAt, removedLines, ...Array(insertedLines).fill(""));
  context.projectedLines[startLine] = null;
  context.blockClasses[startLine] = "block-plain";
  context.lineInlines[startLine] = [];
  context.inlineDigests[startLine] = "";
}

function countNewlines(text) {
  let count = 0;
  for (let index = text.indexOf("\n"); index >= 0; index = text.indexOf("\n", index + 1)) {
    count += 1;
  }
  return count;
}

export function synchronizeProjectionScroll(input, ...layers) {
  // A projection taller than the textarea's padded extent cannot be reached by
  // `-scrollTop` alone; absorb the surplus proportionally so its tail lands in
  // view exactly at the scroll floor.
  const vertical = projectionScrollOffset(input);
  layers.forEach((layer) => {
    layer.style.transform = `translate(${-input.scrollLeft}px, ${-vertical}px)`;
  });
}

export function findMarkdownLink(text, utf16Offset) {
  const expression = /\[([^\]]+)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/gu;
  for (const match of text.matchAll(expression)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    if (utf16Offset >= start && utf16Offset <= end) {
      return { label: match[1], href: match[2] };
    }
  }
  return null;
}

/** Return true when a source offset points at a Markdown task marker. */
export function isMarkdownCheckboxAt(text, utf16Offset) {
  const expression = /^[\t ]*(?:[-*+]|\d+[.)])[\t ]+\[[ xX]\]/gmu;
  for (const match of text.matchAll(expression)) {
    const start = match.index ?? 0;
    if (utf16Offset >= start && utf16Offset <= start + match[0].length) {
      return true;
    }
  }
  return false;
}

function ensureLineElements(container, lineCount) {
  while (container.children.length > lineCount) {
    container.lastElementChild.remove();
  }
  if (container.children.length === lineCount) {
    return;
  }
  const fragment = document.createDocumentFragment();
  for (let index = container.children.length; index < lineCount; index += 1) {
    const line = document.createElement("div");
    line.className = "line";
    fragment.append(line);
  }
  container.append(fragment);
}

function computeBlockClasses(blocks, lineStarts) {
  const classes = Array(lineStarts.length).fill("block-plain");
  for (const block of blocks) {
    let lineIndex = lowerBound(lineStarts, block.startByte);
    while (lineIndex < lineStarts.length && lineStarts[lineIndex] < block.endByte) {
      if (classes[lineIndex] === "block-plain") {
        classes[lineIndex] = `block-${block.kind.replace(":", "-")}`;
      }
      lineIndex += 1;
    }
  }
  return classes;
}

function bucketInlinesByLine(inlines, lineStarts) {
  const buckets = Array.from({ length: lineStarts.length }, () => []);
  for (const span of inlines) {
    if (span.kind.startsWith("marker:") || span.endByte <= span.startByte) {
      continue;
    }
    const firstLine = Math.max(0, upperBound(lineStarts, span.startByte) - 1);
    const lastLine = Math.min(
      lineStarts.length - 1,
      upperBound(lineStarts, span.endByte - 1) - 1,
    );
    for (let lineIndex = firstLine; lineIndex <= lastLine; lineIndex += 1) {
      buckets[lineIndex].push(span);
    }
  }
  return buckets;
}

/** Line-relative inline-span digest so fingerprints track decoration changes. */
function computeInlineDigest(spans, lineStart) {
  let digest = "";
  for (const span of spans) {
    digest += `${span.kind}:${Math.max(0, span.startByte - lineStart)}:${span.endByte - lineStart};`;
  }
  return digest;
}

function lowerBound(values, target) {
  let lower = 0;
  let upper = values.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (values[middle] < target) {
      lower = middle + 1;
    } else {
      upper = middle;
    }
  }
  return lower;
}

function upperBound(values, target) {
  let lower = 0;
  let upper = values.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (values[middle] <= target) {
      lower = middle + 1;
    } else {
      upper = middle;
    }
  }
  return lower;
}

function inlineRanges(line, inlines) {
  const ranges = [];
  for (const span of inlines) {
    if (span.kind.startsWith("marker:")) {
      continue;
    }
    const displayRange = computeDisplayRange(line, span);
    if (!displayRange) {
      continue;
    }
    const start = utf8ByteToUtf16(line.text, displayRange.start);
    const end = utf8ByteToUtf16(line.text, displayRange.end);
    ranges.push({
      start,
      end: Math.max(start + 1, end),
      kind: span.kind,
      sourceStart: span.startByte,
      sourceEnd: span.endByte,
    });
  }
  return ranges;
}

function computeDisplayRange(line, span) {
  if (line.displayToSource.length > 0) {
    const bytes = line.displayToSource
      .filter(([, source]) => source >= span.startByte && source < span.endByte)
      .map(([display]) => display);
    return bytes.length > 0 ? { start: Math.min(...bytes), end: Math.max(...bytes) + 1 } : null;
  }
  let displayByte = 0;
  let start;
  let end;
  for (const segment of line.segments) {
    const displayLength = segment.kind === "visible"
      ? segment.endByte - segment.startByte
      : PROJECTION_ENCODER.encode(segment.replacement).byteLength;
    const overlapStart = Math.max(segment.startByte, span.startByte);
    const overlapEnd = Math.min(segment.endByte, span.endByte);
    if (overlapStart < overlapEnd && segment.kind !== "hidden") {
      const offset = segment.kind === "visible" ? overlapStart - segment.startByte : 0;
      start ??= displayByte + offset;
      end = segment.kind === "visible"
        ? displayByte + overlapEnd - segment.startByte
        : displayByte + displayLength;
    }
    displayByte += displayLength;
  }
  return start === undefined ? null : { start, end };
}

function renderLine(element, text, ranges) {
  const boundaries = new Set([0, text.length]);
  ranges.forEach(({ start, end }) => {
    boundaries.add(Math.max(0, Math.min(start, text.length)));
    boundaries.add(Math.max(0, Math.min(end, text.length)));
  });
  const ordered = [...boundaries].sort((left, right) => left - right);
  const fragment = document.createDocumentFragment();
  for (let index = 0; index < ordered.length - 1; index += 1) {
    const start = ordered[index];
    const end = ordered[index + 1];
    const value = text.slice(start, end);
    const active = ranges.filter((range) => range.start <= start && range.end >= end);
    const kind = INLINE_PRIORITY.find((candidate) => active.some((range) => range.kind === candidate));
    if (!kind) {
      fragment.append(document.createTextNode(value));
      continue;
    }
    const span = document.createElement("span");
    span.className = `inline-${kind}`;
    span.textContent = value;
    const sourceRange = active.find((range) => range.kind === kind);
    span.dataset.sourceStart = String(sourceRange.sourceStart);
    span.dataset.sourceEnd = String(sourceRange.sourceEnd);
    fragment.append(span);
  }
  if (text.length === 0) {
    fragment.append(document.createElement("br"));
  }
  element.replaceChildren(fragment);
}
