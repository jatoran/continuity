import {
  positionToUtf16Offset,
  selectionFromInput,
  utf16OffsetToPosition,
  utf16ToUtf8Byte,
} from "./coordinates.js";

/** Replace the semantic primary selection without discarding secondary selections. */
export function selectionsWithInputPrimary(snapshot, input) {
  const inputPrimary = selectionFromInput(snapshot.text, input);
  const enginePrimary = snapshot.selections[0];
  const primary = enginePrimary && hasSameEndpoints(enginePrimary, inputPrimary)
    ? enginePrimary
    : inputPrimary;
  return [primary, ...snapshot.selections.slice(1)];
}

/** Add and activate a visual-row caret above or below the outermost caret. */
export function addVerticalCaret(snapshot, direction, input) {
  const heads = snapshot.selections.map(({ head }) => head);
  const edge = direction < 0
    ? heads.reduce((left, right) => positionOrder(left, right) <= 0 ? left : right)
    : heads.reduce((left, right) => positionOrder(left, right) >= 0 ? left : right);
  const added = adjacentVisualRowCaret(snapshot.text, edge, direction, input);
  return added ? prioritizeSelection(added, snapshot.selections) : snapshot.selections;
}

/**
 * Collapse to a single caret one *visible* row above or below the primary
 * head. A soft-wrapped source line occupies several rows, so this walks
 * wrapped rows like the platform arrow keys rather than whole source lines,
 * and keeps the caret's horizontal position across the move. Falls back to the
 * adjacent source line when the row cannot be measured (no layout yet).
 */
export function verticalCaretSelections(snapshot, direction, input) {
  const head = snapshot.selections[0]?.head;
  if (!head) {
    return snapshot.selections;
  }
  const moved = adjacentVisualRowCaret(snapshot.text, head, direction, input);
  return moved ? [moved] : snapshot.selections;
}

function adjacentVisualRowCaret(text, from, direction, input) {
  const fromOffset = positionToUtf16Offset(text, from);
  const visualOffset = input ? adjacentVisualRowOffset(input, fromOffset, direction) : null;
  return visualOffset === null
    ? adjacentSourceLineCaret(text, from, direction)
    : caretAtUtf16Offset(text, visualOffset);
}

function adjacentSourceLineCaret(text, edge, direction) {
  const lines = text.split("\n");
  const targetLine = edge.line + direction;
  if (targetLine < 0 || targetLine >= lines.length) {
    return null;
  }
  const line = lines[targetLine] ?? "";
  const byteInLine = Math.min(edge.byteInLine, utf16ToUtf8Byte(line, line.length));
  return caret(targetLine, clampUtf8Boundary(line, byteInLine));
}

/** Add and activate a modifier-click caret captured through native hit-testing. */
export function addPointerCaret(text, selectionsBeforePointer, input) {
  return prioritizeSelection(selectionFromInput(text, input), selectionsBeforePointer);
}

function adjacentVisualRowOffset(input, edgeOffset, direction) {
  const mirror = createTextareaMirror(input);
  const textNode = document.createTextNode(`${input.value}\u200b`);
  mirror.append(textNode);
  document.body.append(mirror);
  const range = document.createRange();
  const maximumOffset = input.value.length;
  const pointAt = (offset) => {
    const normalized = normalizeUtf16Boundary(input.value, Math.max(0, Math.min(offset, maximumOffset)));
    range.setStart(textNode, normalized);
    range.collapse(true);
    const bounds = range.getBoundingClientRect();
    return { offset: normalized, left: bounds.left, top: bounds.top };
  };
  const current = pointAt(edgeOffset);
  const lineHeight = parseFloat(getComputedStyle(input).lineHeight) || 24;
  const desiredTop = current.top + direction * lineHeight;
  const rowTolerance = Math.max(1, lineHeight * 0.25);
  const rowStart = lowerBoundOffset(maximumOffset, (offset) => pointAt(offset).top >= desiredTop - rowTolerance);
  const first = pointAt(rowStart);
  if (Math.abs(first.top - desiredTop) > rowTolerance) {
    mirror.remove();
    return null;
  }
  const rowEnd = lowerBoundOffset(maximumOffset, (offset) => pointAt(offset).top > first.top + rowTolerance);
  const horizontal = lowerBoundRange(rowStart, rowEnd, (offset) => pointAt(offset).left >= current.left);
  const candidates = [pointAt(horizontal), pointAt(Math.max(rowStart, horizontal - 1))];
  const closest = candidates.reduce((left, right) => (
    Math.abs(left.left - current.left) <= Math.abs(right.left - current.left) ? left : right
  ));
  mirror.remove();
  return closest.offset;
}

function createTextareaMirror(input) {
  const style = getComputedStyle(input);
  const mirror = document.createElement("div");
  mirror.style.cssText = [
    "position:fixed",
    "inset:0 auto auto -10000px",
    "visibility:hidden",
    "pointer-events:none",
    "white-space:pre-wrap",
    "overflow-wrap:anywhere",
    "box-sizing:border-box",
  ].join(";");
  for (const property of [
    "font", "letterSpacing", "lineHeight", "padding", "border", "tabSize",
    "textIndent", "wordSpacing",
  ]) {
    mirror.style[property] = style[property];
  }
  // `clientWidth` excludes a visible textarea scrollbar. Using the border-box
  // width makes wrapped carets drift when the mirror wraps against more space.
  mirror.style.width = `${input.clientWidth}px`;
  return mirror;
}

function lowerBoundOffset(maximumOffset, predicate) {
  return lowerBoundRange(0, maximumOffset, predicate);
}

function lowerBoundRange(start, end, predicate) {
  let low = start;
  let high = end;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    if (predicate(middle)) {
      high = middle;
    } else {
      low = middle + 1;
    }
  }
  return low;
}

function normalizeUtf16Boundary(text, offset) {
  const code = text.charCodeAt(offset);
  return code >= 0xDC00 && code <= 0xDFFF ? offset - 1 : offset;
}

function caretAtUtf16Offset(text, offset) {
  const position = utf16OffsetToPosition(text, offset);
  return { anchor: position, head: position, kind: "caret" };
}

function prioritizeSelection(added, selections) {
  return [added, ...selections.filter((selection) => !isSameSelection(selection, added))];
}

function isSameSelection(left, right) {
  return left.kind === right.kind
    && positionOrder(left.anchor, right.anchor) === 0
    && positionOrder(left.head, right.head) === 0;
}

function hasSameEndpoints(left, right) {
  return positionOrder(left.anchor, right.anchor) === 0
    && positionOrder(left.head, right.head) === 0;
}

function clampUtf8Boundary(line, requestedByte) {
  let utf16Offset = 0;
  let bytes = 0;
  for (const scalar of line) {
    const scalarBytes = utf16ToUtf8Byte(scalar, scalar.length);
    if (bytes + scalarBytes > requestedByte) {
      break;
    }
    bytes += scalarBytes;
    utf16Offset += scalar.length;
  }
  return utf16ToUtf8Byte(line, utf16Offset);
}

function caret(line, byteInLine) {
  const position = { line, byteInLine };
  return { anchor: position, head: position, kind: "caret" };
}

function positionOrder(left, right) {
  return left.line - right.line || left.byteInLine - right.byteInLine;
}
