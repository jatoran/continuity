import assert from "node:assert/strict";
import test from "node:test";
import {
  positionToUtf16Offset,
  selectedLines,
  sourceLineStarts,
  utf16OffsetToPosition,
  utf16ToUtf8Byte,
  utf8ByteToUtf16,
} from "./node_modules/@continuity-editor/editor/src/coordinates.js";

test("UTF-8 and UTF-16 coordinates round-trip Unicode source lines", () => {
  const text = "ascii\né🙂中\nend";
  const offsets = [0, 5, 6, 7, 9, 10, 11, text.length];
  for (const offset of offsets) {
    const position = utf16OffsetToPosition(text, offset);
    assert.equal(positionToUtf16Offset(text, position), offset);
  }
  assert.deepEqual(sourceLineStarts(text), [0, 6, 16]);
});

test("byte conversion stops before a partial scalar", () => {
  const text = "aé🙂中";
  assert.equal(utf16ToUtf8Byte(text, text.length), 10);
  assert.equal(utf8ByteToUtf16(text, 2), 1);
  assert.equal(utf8ByteToUtf16(text, 3), 2);
  assert.equal(utf8ByteToUtf16(text, 6), 2);
  assert.equal(utf8ByteToUtf16(text, 7), 4);
});

test("only selection endpoint lines reveal source", () => {
  const text = "alpha\nbeta\ngamma\ndelta";
  const input = { selectionStart: 2, selectionEnd: text.length - 2 };
  assert.deepEqual([...selectedLines(text, input)], [0, 3]);
});
