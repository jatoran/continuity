import { utf8ByteToUtf16 } from "./coordinates.js";

/**
 * DOM construction for one projected line: inline-span slicing and the
 * pipe-table row variant. Split out of `projection.js` so that file keeps to
 * viewport realization and reconciliation.
 */

const INLINE_PRIORITY = ["link", "checkbox", "code", "strong", "emphasis", "strikethrough"];
const PROJECTION_ENCODER = new TextEncoder();

/** Replace `element`'s children with `text` sliced into inline-styled spans. */
export function renderLine(element, text, ranges) {
  const fragment = buildInlineFragment(text, ranges, 0, text.length);
  if (text.length === 0) {
    fragment.append(document.createElement("br"));
  }
  element.replaceChildren(fragment);
}

/**
 * Render one pipe-table row: a cell element per column, each holding exactly
 * the slice of the projected text between two hidden pipes. The concatenated
 * cell text is the line text, so hit-testing walks the same text nodes in the
 * same order as a plain line. The delimiter row renders empty cells (its
 * bytes are all hidden) and collapses through CSS.
 */
export function renderTableRow(element, text, ranges, row) {
  const fragment = document.createDocumentFragment();
  row.cells.forEach((cell, column) => {
    const cellElement = document.createElement("span");
    cellElement.className = `table-cell table-align-${row.alignments[column] ?? "left"}`;
    cellElement.append(buildInlineFragment(text, ranges, cell.start, cell.end));
    fragment.append(cellElement);
  });
  element.replaceChildren(fragment);
}

/** Inline-styled spans for `text.slice(start, end)`, in document order. */
function buildInlineFragment(text, ranges, start, end) {
  const boundaries = new Set([start, end]);
  ranges.forEach((range) => {
    boundaries.add(Math.max(start, Math.min(range.start, end)));
    boundaries.add(Math.max(start, Math.min(range.end, end)));
  });
  const ordered = [...boundaries].sort((left, right) => left - right);
  const fragment = document.createDocumentFragment();
  for (let index = 0; index < ordered.length - 1; index += 1) {
    const sliceStart = ordered[index];
    const sliceEnd = ordered[index + 1];
    if (sliceEnd <= sliceStart) continue;
    const value = text.slice(sliceStart, sliceEnd);
    const active = ranges.filter((range) => range.start <= sliceStart && range.end >= sliceEnd);
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
  return fragment;
}

/** Inline decoration spans of one line mapped onto its projected UTF-16 text. */
export function inlineRanges(line, inlines) {
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
