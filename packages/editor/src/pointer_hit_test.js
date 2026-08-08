import { utf16ToUtf8Byte } from "./coordinates.js";

const POINTER_ENCODER = new TextEncoder();
const graphemeSegmenter = typeof Intl?.Segmenter === "function"
  ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
  : null;

/** Find the closest source position from the projection's measured glyph geometry. */
export function projectionPointToPosition(container, context, clientX, clientY) {
  const element = closestLineElement(container, clientY);
  if (!element) return null;
  const lineIndex = Number.parseInt(element.dataset.line ?? "", 10);
  if (!Number.isInteger(lineIndex)) return null;
  // While an IME composition previews this line the rendered glyphs are the
  // live textarea text but the engine `sourceLines` mirror is still the
  // pre-composition text; hit-test against the live line instead so the tap
  // lands on the character under the finger, not a stale-mapped byte.
  const composition = context.compositionLine;
  const sourceText = composition && composition.index === lineIndex
    ? composition.text
    : context.sourceLines[lineIndex] ?? "";
  const report = context.projectedLines[lineIndex];
  const displayOffset = measuredTextOffset(element, clientX, clientY);
  const isProjected = element.dataset.detailed === "true"
    && element.dataset.sourceVisible !== "true"
    && Boolean(report);
  const sourceByte = isProjected
    ? displayByteToSource(report, utf16ToUtf8Byte(report.text, displayOffset))
    : utf16ToUtf8Byte(sourceText, displayOffset);
  const sourceLineBytes = POINTER_ENCODER.encode(sourceText).byteLength;
  const reportLineStart = report?.segments[0]?.startByte ?? 0;
  const byteInLine = isProjected
    ? Math.max(0, Math.min(sourceLineBytes, sourceByte - reportLineStart))
    : Math.max(0, Math.min(sourceLineBytes, sourceByte));
  return { line: lineIndex, byteInLine };
}

function closestLineElement(container, clientY) {
  const children = container.children;
  if (children.length === 0) return null;
  let lower = 0;
  let upper = children.length;
  while (lower < upper) {
    const middle = lower + Math.floor((upper - lower) / 2);
    if (children[middle].getBoundingClientRect().bottom < clientY) lower = middle + 1;
    else upper = middle;
  }
  const candidates = [children[Math.min(lower, children.length - 1)], children[lower - 1]]
    .filter(Boolean);
  return candidates.reduce((closest, candidate) => (
    verticalDistance(candidate.getBoundingClientRect(), clientY)
      < verticalDistance(closest.getBoundingClientRect(), clientY)
      ? candidate
      : closest
  ));
}

function measuredTextOffset(element, clientX, clientY) {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  const candidates = [];
  let textOffset = 0;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const value = node.nodeValue ?? "";
    for (const { start, end } of graphemeBoundaries(value)) {
      const range = document.createRange();
      range.setStart(node, start);
      range.setEnd(node, end);
      for (const rect of range.getClientRects()) {
        if (rect.width > 0 || rect.height > 0) {
          candidates.push({ start: textOffset + start, end: textOffset + end, rect });
        }
      }
    }
    textOffset += value.length;
  }
  if (candidates.length === 0) return 0;
  const closest = candidates.reduce((current, candidate) => (
    candidateDistance(candidate.rect, clientX, clientY)
      < candidateDistance(current.rect, clientX, clientY)
      ? candidate
      : current
  ));
  return clientX < closest.rect.left + closest.rect.width / 2 ? closest.start : closest.end;
}

function graphemeBoundaries(value) {
  if (graphemeSegmenter) {
    return [...graphemeSegmenter.segment(value)].map(({ index, segment }) => ({
      start: index,
      end: index + segment.length,
    }));
  }
  const boundaries = [];
  for (let index = 0; index < value.length;) {
    const width = value.codePointAt(index) > 0xFFFF ? 2 : 1;
    boundaries.push({ start: index, end: index + width });
    index += width;
  }
  return boundaries;
}

function candidateDistance(rect, clientX, clientY) {
  const vertical = verticalDistance(rect, clientY);
  const horizontal = clientX < rect.left
    ? rect.left - clientX
    : clientX > rect.right ? clientX - rect.right : 0;
  return vertical * vertical * 1_000_000 + horizontal * horizontal;
}

function verticalDistance(rect, clientY) {
  if (clientY < rect.top) return rect.top - clientY;
  if (clientY > rect.bottom) return clientY - rect.bottom;
  return 0;
}

function displayByteToSource(line, requestedDisplayByte) {
  if (line.displayToSource.length > 0) {
    let source = line.displayToSource[0]?.[1] ?? line.segments[0]?.startByte ?? 0;
    for (const [displayByte, sourceByte] of line.displayToSource) {
      if (displayByte > requestedDisplayByte) break;
      source = sourceByte;
      if (displayByte === requestedDisplayByte) return source;
    }
    return source;
  }
  let displayCursor = 0;
  let sourceAtBoundary = line.segments[0]?.startByte ?? 0;
  for (const segment of line.segments) {
    const displayLength = segment.kind === "visible"
      ? segment.endByte - segment.startByte
      : POINTER_ENCODER.encode(segment.replacement).byteLength;
    if (displayLength === 0) {
      sourceAtBoundary = segment.endByte;
      continue;
    }
    if (requestedDisplayByte < displayCursor + displayLength) {
      return segment.kind === "visible"
        ? segment.startByte + requestedDisplayByte - displayCursor
        : segment.startByte;
    }
    displayCursor += displayLength;
    sourceAtBoundary = segment.endByte;
    if (requestedDisplayByte === displayCursor) continue;
  }
  return sourceAtBoundary;
}
