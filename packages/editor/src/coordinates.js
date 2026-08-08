export function utf16OffsetToPosition(text, requestedOffset) {
  const offset = Math.max(0, Math.min(requestedOffset, text.length));
  let line = 0;
  let lineStart = 0;
  for (let index = 0; index < offset; index += 1) {
    if (text.charCodeAt(index) === 10) {
      line += 1;
      lineStart = index + 1;
    }
  }
  return {
    line,
    byteInLine: countUtf8Bytes(text, lineStart, offset),
  };
}

export function positionToUtf16Offset(text, position) {
  const requestedLine = Math.max(0, position.line);
  let line = 0;
  let lineStart = 0;
  while (line < requestedLine) {
    const newline = text.indexOf("\n", lineStart);
    if (newline < 0) break;
    lineStart = newline + 1;
    line += 1;
  }
  const newline = text.indexOf("\n", lineStart);
  const lineEnd = newline < 0 ? text.length : newline;
  return lineStart + utf8ByteToUtf16Range(
    text, lineStart, lineEnd, Math.max(0, position.byteInLine),
  );
}

export function selectionFromInput(text, input) {
  const start = utf16OffsetToPosition(text, input.selectionStart ?? 0);
  const end = utf16OffsetToPosition(text, input.selectionEnd ?? 0);
  const isBackward = input.selectionDirection === "backward";
  return {
    anchor: isBackward ? end : start,
    head: isBackward ? start : end,
    kind: "caret",
  };
}

export function applySelectionToInput(text, selection, input) {
  const anchor = positionToUtf16Offset(text, selection.anchor);
  const head = positionToUtf16Offset(text, selection.head);
  const start = Math.min(anchor, head);
  const end = Math.max(anchor, head);
  input.setSelectionRange(start, end, head < anchor ? "backward" : "forward");
}

export function selectedLines(text, input) {
  const start = utf16OffsetToPosition(text, input.selectionStart ?? 0).line;
  const end = utf16OffsetToPosition(text, input.selectionEnd ?? 0).line;
  return new Set(start === end ? [start] : [start, end]);
}

export function sourceLineStarts(text) {
  const starts = [0];
  let bytes = 0;
  for (let index = 0; index < text.length;) {
    const code = text.charCodeAt(index);
    if (code === 10) {
      bytes += 1;
      starts.push(bytes);
      index += 1;
      continue;
    }
    const width = utf8Width(text, index, text.length);
    bytes += width;
    index += width === 4 ? 2 : 1;
  }
  return starts;
}

export function utf8ByteToUtf16(text, requestedByte) {
  return utf8ByteToUtf16Range(text, 0, text.length, Math.max(0, requestedByte));
}

export function utf16ToUtf8Byte(text, offset) {
  return countUtf8Bytes(text, 0, Math.max(0, Math.min(offset, text.length)));
}

function utf8ByteToUtf16Range(text, start, end, requestedByte) {
  let bytes = 0;
  let index = start;
  while (index < end) {
    const width = utf8Width(text, index, end);
    if (bytes + width > requestedByte) break;
    bytes += width;
    index += width === 4 ? 2 : 1;
  }
  return index - start;
}

function countUtf8Bytes(text, start, end) {
  let bytes = 0;
  for (let index = start; index < end;) {
    const width = utf8Width(text, index, end);
    bytes += width;
    index += width === 4 ? 2 : 1;
  }
  return bytes;
}

function utf8Width(text, index, end) {
  const code = text.charCodeAt(index);
  if (code <= 0x7F) return 1;
  if (code <= 0x7FF) return 2;
  if (code >= 0xD800 && code <= 0xDBFF && index + 1 < end) {
    const following = text.charCodeAt(index + 1);
    if (following >= 0xDC00 && following <= 0xDFFF) {
      return 4;
    }
  }
  return 3;
}
