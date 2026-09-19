import assert from "node:assert/strict";
import test from "node:test";
import {
  computeCellRanges,
  computeTableRows,
  fitColumnWidths,
  parseAlignments,
} from "./node_modules/@continuity-editor/editor/src/projection_tables.js";

const ENCODER = new TextEncoder();

/**
 * Build the projected line report the engine emits for one table line: every
 * unescaped `|` becomes a hidden segment, everything else stays visible.
 */
function projectTableLine(sourceText, lineStart = 0) {
  const bytes = ENCODER.encode(sourceText);
  const segments = [];
  let text = "";
  let runStart = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    if (bytes[index] !== 0x7c || (index > 0 && bytes[index - 1] === 0x5c)) continue;
    if (index > runStart) {
      segments.push({ kind: "visible", startByte: lineStart + runStart, endByte: lineStart + index, replacement: "" });
      text += new TextDecoder().decode(bytes.subarray(runStart, index));
    }
    segments.push({ kind: "hidden", startByte: lineStart + index, endByte: lineStart + index + 1, replacement: "" });
    runStart = index + 1;
  }
  if (runStart < bytes.length) {
    segments.push({ kind: "visible", startByte: lineStart + runStart, endByte: lineStart + bytes.length, replacement: "" });
    text += new TextDecoder().decode(bytes.subarray(runStart));
  }
  return { text, wrapIndentByteEnd: 0, sourceToDisplay: [], displayToSource: [], segments };
}

const measure = (text) => text.length * 8;
const OPTIONS = { measure, fontSizePx: 16, availableWidth: 0 };

test("cells split at hidden pipes and concatenate back to the projected text", () => {
  const source = "| # | TEAM NAME | EMAIL |";
  const line = projectTableLine(source);
  const cells = computeCellRanges(line, source, 3);
  assert.deepEqual(
    cells.map(({ start, end }) => line.text.slice(start, end)),
    [" # ", " TEAM NAME ", " EMAIL "],
  );
  assert.equal(cells.map(({ start, end }) => line.text.slice(start, end)).join(""), line.text);
});

test("leading pipe adds no empty cell and surplus text folds into the last column", () => {
  const source = "| a | b | c | d |  ";
  const line = projectTableLine(source);
  const cells = computeCellRanges(line, source, 3);
  assert.equal(cells.length, 3);
  assert.equal(line.text.slice(cells[2].start, cells[2].end), " c  d   ");
  assert.equal(cells.map(({ start, end }) => line.text.slice(start, end)).join(""), line.text);
});

test("short rows pad with empty trailing cells and escaped pipes stay inside a cell", () => {
  const source = "a \\| b | c";
  const line = projectTableLine(source);
  const cells = computeCellRanges(line, source, 4);
  assert.equal(cells.length, 4);
  assert.equal(line.text.slice(cells[0].start, cells[0].end), "a \\| b ");
  assert.equal(line.text.slice(cells[1].start, cells[1].end), " c");
  assert.deepEqual(cells.slice(2).map(({ start, end }) => end - start), [0, 0]);
});

test("cell boundaries are UTF-16 offsets even after multi-byte text", () => {
  const source = "| é🙂中 | b |";
  const line = projectTableLine(source);
  const cells = computeCellRanges(line, source, 2);
  assert.equal(line.text.slice(cells[0].start, cells[0].end), " é🙂中 ");
  assert.equal(line.text.slice(cells[1].start, cells[1].end), " b ");
});

test("delimiter alignments parse left, center, and right", () => {
  assert.deepEqual(parseAlignments("| :--- | :---: | ---: | --- |"), ["left", "center", "right", "left"]);
  assert.deepEqual(parseAlignments("---|:-:"), ["left", "center"]);
});

test("column widths floor, cap, pad, and shrink to fit the pane", () => {
  const options = { fontSizePx: 10, availableWidth: 0 };
  // 3em floor, 16em cap, 0.8em padding at 10px per em.
  assert.deepEqual(fitColumnWidths([0, 50, 1000], options), [30, 58, 160]);
  const fitted = fitColumnWidths([100, 100, 100], { fontSizePx: 10, availableWidth: 200 });
  assert.equal(fitted.reduce((sum, width) => sum + width, 0) <= 200, true);
  assert.ok(fitted.every((width) => width >= 30));
  // Columns already at the floor cannot shrink further.
  assert.deepEqual(fitColumnWidths([0, 0], { fontSizePx: 10, availableWidth: 20 }), [30, 30]);
});

test("a table run yields grid rows sharing one template, with the delimiter collapsed", () => {
  const sourceLines = ["| # | NAME |", "| --- | :---: |", "| 1 | Carolla's best buddy |", "| 2 | Bend the Knee |", "after"];
  let byte = 0;
  const projectedLines = sourceLines.map((line, index) => {
    const report = index < 4 ? projectTableLine(line, byte) : null;
    byte += ENCODER.encode(line).length + 1;
    return report;
  });
  const context = {
    blockClasses: ["block-pipeTable", "block-pipeTable", "block-pipeTable", "block-pipeTable", "block-paragraph"],
    projectedLines,
    sourceLines,
  };
  const rows = computeTableRows(context, OPTIONS);
  assert.equal(rows.length, 4);
  assert.equal(rows[4], undefined);
  assert.equal(rows[0].isHeader, true);
  assert.equal(rows[1].isDelimiter, true);
  assert.equal(rows[2].isHeader, false);
  assert.deepEqual(rows[0].alignments, ["left", "center"]);
  assert.equal(new Set(rows.map((row) => row.template)).size, 1, "every row shares the template");
  const widths = rows[0].template.split(" ").map((value) => Number.parseInt(value, 10));
  // Widest content per column ("Carolla's best buddy" = 20 chars * 8px) plus padding.
  assert.equal(widths[1], Math.ceil(20 * 8 + 0.8 * 16));
  assert.equal(widths[0], 3 * 16, "the narrow `#` column sits at the floor");
});

test("a table run without a delimiter row and an unprojected line are left alone", () => {
  const sourceLines = ["| a | b |", "| 1 | 2 |"];
  const projectedLines = sourceLines.map((line) => projectTableLine(line));
  const rows = computeTableRows({ blockClasses: ["block-pipeTable", "block-pipeTable"], projectedLines, sourceLines }, OPTIONS);
  assert.deepEqual(rows, []);
  const withDelimiter = ["| a | b |", "| - | - |", "| 1 | 2 |"];
  const partial = withDelimiter.map((line, index) => (index === 2 ? null : projectTableLine(line)));
  const partialRows = computeTableRows(
    { blockClasses: Array(3).fill("block-pipeTable"), projectedLines: partial, sourceLines: withDelimiter },
    OPTIONS,
  );
  assert.ok(partialRows[0] && partialRows[1]);
  assert.equal(partialRows[2], undefined, "an unprojected line renders as source until projected");
});

test("an unprojected pipe line under edit bridges the run so lower rows keep their grid", () => {
  const sourceLines = ["| a | b |", "| - | - |", "| 1 | typing here", "| 3 | 4 |", "| 5 | 6 |"];
  const projectedLines = sourceLines.map((line, index) => (index === 2 ? null : projectTableLine(line)));
  const blockClasses = ["block-pipeTable", "block-pipeTable", "block-plain", "block-pipeTable", "block-pipeTable"];
  const rows = computeTableRows({ blockClasses, projectedLines, sourceLines }, OPTIONS);
  assert.equal(rows[2], undefined, "the edited line renders as raw source");
  assert.ok(rows[3] && rows[4], "rows below the edited line stay in the table");
  assert.equal(rows[3].template, rows[0].template);
  assert.equal(rows[3].isHeader, false);
});
