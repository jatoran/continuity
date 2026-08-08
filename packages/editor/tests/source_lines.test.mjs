import assert from "node:assert/strict";
import test from "node:test";
import { applySourceEdits } from "./node_modules/@continuity-editor/editor/src/source_lines.js";

test("source-line mirror applies single-line and structural splices", () => {
  const lines = ["alpha", "beta", "gamma"];
  applySourceEdits(lines, [{
    startLine: 1,
    endLine: 1,
    startUtf16InLine: 2,
    endUtf16InLine: 4,
    insertedText: "X\nY",
  }], "alpha\nbeX\nY\ngamma");
  assert.deepEqual(lines, ["alpha", "beX", "Y", "gamma"]);
});

test("source-line mirror honors descending multi-cursor edit order", () => {
  const lines = ["alpha", "beta", "gamma"];
  const edits = [
    { startLine: 2, endLine: 2, startUtf16InLine: 2, endUtf16InLine: 2, insertedText: "!" },
    { startLine: 0, endLine: 0, startUtf16InLine: 2, endUtf16InLine: 2, insertedText: "!" },
  ];
  applySourceEdits(lines, edits, "al!pha\nbeta\nga!mma");
  assert.deepEqual(lines, ["al!pha", "beta", "ga!mma"]);
});

test("source-line mirror safely rebuilds when splice metadata is absent", () => {
  const rebuilt = applySourceEdits(["stale"], [{ insertedText: "" }], "fresh\ntext");
  assert.deepEqual(rebuilt, ["fresh", "text"]);
});
