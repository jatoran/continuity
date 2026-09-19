import { utf8ByteToUtf16 } from "./coordinates.js";

/**
 * Pipe-table layout for the browser projection.
 *
 * The engine already classifies table lines (`block-pipeTable`) and hides every
 * `|` byte through the shared display map, so a projected table line arrives as
 * its cell texts run together. This module turns one contiguous run of table
 * lines into a column grid: each line keeps its own DOM element (the projection
 * holds one element per source line), the grid template is shared by every
 * row of the table, and cells are split at the display offsets where the hidden
 * pipes sat. Cell texts concatenate back to the exact projected line text, so
 * grapheme hit-testing and caret measurement keep walking text nodes in order.
 *
 * Column widths follow the Windows renderer's policy: the widest cell wins, a
 * column is capped so prose wraps instead of stretching the pane, and when the
 * capped columns still overflow the pane they shrink proportionally (never
 * below the minimum) so the table fits and wraps rather than scrolling.
 *
 * Text measurement is injected so the pure splitting and sizing logic runs
 * under node without a canvas.
 */

export const TABLE_BLOCK_CLASS = "block-pipeTable";
const DELIMITER_LINE = /^[\t ]*\|?[\t ]*:?-+:?[\t ]*(?:\|[\t ]*:?-+:?[\t ]*)*\|?[\t ]*$/u;
const MIN_COLUMN_EM = 3;
const MAX_COLUMN_EM = 16;
const CELL_PADDING_EM = 0.8;

/**
 * Compute a table row descriptor for every projected line that sits inside a
 * pipe table. Returns a sparse array indexed by source line; lines outside a
 * table, unprojected lines, and runs without a delimiter row are `undefined`.
 *
 * `options.measure(text)` returns the advance of `text` in CSS pixels;
 * `options.fontSizePx` scales the column caps; `options.availableWidth` is the
 * pane width the table must fit inside (0 disables the fit pass).
 */
export function computeTableRows(context, options) {
  const rows = [];
  const { blockClasses, projectedLines, sourceLines } = context;
  let index = 0;
  while (index < blockClasses.length) {
    if (blockClasses[index] !== TABLE_BLOCK_CLASS) {
      index += 1;
      continue;
    }
    let end = index;
    while (end < blockClasses.length && isTableRunLine(end, blockClasses, projectedLines, sourceLines)) {
      end += 1;
    }
    layoutTable(rows, index, end, projectedLines, sourceLines, options);
    index = end;
  }
  return rows;
}

/**
 * Whether `index` continues a table run. A line being edited has its
 * projection invalidated (`block-plain`, no projected line) until the idle
 * pass re-projects it; if that split the run, every row below the caret would
 * drop to run-together text on each keystroke. An unprojected line that still
 * reads as a pipe row bridges the run instead (it renders as raw source
 * itself, but keeps the rows below it in the grid).
 */
function isTableRunLine(index, blockClasses, projectedLines, sourceLines) {
  if (blockClasses[index] === TABLE_BLOCK_CLASS) return true;
  return !projectedLines[index] && (sourceLines[index] ?? "").includes("|");
}

function layoutTable(rows, start, end, projectedLines, sourceLines, options) {
  let delimiterLine = -1;
  for (let line = start; line < end; line += 1) {
    if (DELIMITER_LINE.test(sourceLines[line] ?? "")) {
      delimiterLine = line;
      break;
    }
  }
  if (delimiterLine < 0) return;
  const alignments = parseAlignments(sourceLines[delimiterLine]);
  const columnCount = Math.max(1, alignments.length);
  const widths = Array(columnCount).fill(0);
  const pending = [];
  for (let line = start; line < end; line += 1) {
    if (line === delimiterLine) {
      pending.push({ line, cells: null });
      continue;
    }
    const projected = projectedLines[line];
    if (!projected) continue;
    const cells = computeCellRanges(projected, sourceLines[line] ?? "", columnCount);
    cells.forEach((cell, column) => {
      const text = projected.text.slice(cell.start, cell.end).trim();
      if (text.length === 0) return;
      widths[column] = Math.max(widths[column], options.measure(text));
    });
    pending.push({ line, cells });
  }
  const columns = fitColumnWidths(widths, options);
  const template = columns.map((width) => `${width}px`).join(" ");
  const signature = `${template}${alignments.join("")}`;
  for (const entry of pending) {
    rows[entry.line] = {
      tableStart: start,
      isHeader: entry.line === start,
      isDelimiter: entry.cells === null,
      cells: entry.cells ?? Array.from({ length: columnCount }, () => ({ start: 0, end: 0 })),
      alignments,
      template,
      signature,
    };
  }
}

/** Column alignments from a delimiter row: `:--` left, `:-:` center, `--:` right. */
export function parseAlignments(delimiterText) {
  const body = (delimiterText ?? "").trim().replace(/^\|/u, "").replace(/\|$/u, "");
  return body.split("|").map((cell) => {
    const trimmed = cell.trim();
    const left = trimmed.startsWith(":");
    const right = trimmed.endsWith(":");
    if (left && right) return "center";
    if (right) return "right";
    return "left";
  });
}

/**
 * Split one projected table line into `columnCount` cell ranges (UTF-16 offsets
 * into `line.text`). Boundaries are the display offsets of the hidden `|`
 * bytes; a leading pipe contributes no empty first cell and any text past the
 * last column (a trailing pipe's whitespace, surplus cells) folds into the last
 * column so no projected text is dropped. Missing cells are padded empty.
 */
export function computeCellRanges(line, sourceText, columnCount) {
  const lineStart = line.segments[0]?.startByte ?? 0;
  const boundaries = [];
  let displayByte = 0;
  for (const segment of line.segments) {
    if (segment.kind === "hidden") {
      const isPipe = segment.endByte - segment.startByte === 1
        && sourceText.charAt(utf8ByteToUtf16(sourceText, segment.startByte - lineStart)) === "|";
      if (isPipe) boundaries.push(utf8ByteToUtf16(line.text, displayByte));
      continue;
    }
    displayByte += segment.kind === "visible"
      ? segment.endByte - segment.startByte
      : new TextEncoder().encode(segment.replacement).byteLength;
  }
  const textLength = line.text.length;
  const interior = boundaries.filter((offset) => offset > 0 && offset < textLength);
  const cells = [];
  let cursor = 0;
  for (const boundary of interior) {
    cells.push({ start: cursor, end: boundary });
    cursor = boundary;
  }
  cells.push({ start: cursor, end: textLength });
  if (cells.length > columnCount) {
    const merged = cells.slice(0, columnCount);
    merged[columnCount - 1] = { start: merged[columnCount - 1].start, end: textLength };
    return merged;
  }
  while (cells.length < columnCount) cells.push({ start: textLength, end: textLength });
  return cells;
}

/**
 * Final pixel width per column: measured content plus padding, floored at the
 * minimum, capped at the maximum, then shrunk proportionally (above the
 * minimum) when the row would overflow the available width.
 */
export function fitColumnWidths(measuredWidths, options) {
  const em = options.fontSizePx > 0 ? options.fontSizePx : 16;
  const minimum = MIN_COLUMN_EM * em;
  const maximum = MAX_COLUMN_EM * em;
  const padding = CELL_PADDING_EM * em;
  let columns = measuredWidths.map((width) => (
    Math.min(maximum, Math.max(minimum, Math.ceil(width + padding)))
  ));
  const available = options.availableWidth ?? 0;
  const total = columns.reduce((sum, width) => sum + width, 0);
  if (available > 0 && total > available) {
    const flexible = columns.reduce((sum, width) => sum + Math.max(0, width - minimum), 0);
    const excess = total - available;
    if (flexible > 0) {
      const ratio = Math.max(0, 1 - excess / flexible);
      columns = columns.map((width) => Math.floor(minimum + (width - minimum) * ratio));
    }
  }
  return columns;
}
