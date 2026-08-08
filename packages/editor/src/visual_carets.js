import { utf8ByteToUtf16 } from "./coordinates.js";
import { projectionLineLayout } from "./projection.js";

const CARET_ENCODER = new TextEncoder();

/** Append collapsed engine selections against the visible projection geometry. */
export function appendVisualCarets(fragment, projection, snapshot, containerBounds) {
  let primaryCaret = null;
  snapshot.selections.forEach((selection, index) => {
    if (!isCollapsed(selection)) return;
    const layout = projectionLineLayout(projection, selection.head.line);
    const point = layout ? measureSelectionHead(layout, selection.head.byteInLine) : null;
    if (!point) return;
    const caret = document.createElement("span");
    caret.className = index === 0
      ? "visual-caret primary-caret"
      : "visual-caret secondary-caret";
    caret.style.transform = `translate(${point.left - containerBounds.left}px, ${point.top - containerBounds.top}px)`;
    caret.style.height = `${point.height}px`;
    fragment.append(caret);
    if (index === 0) primaryCaret = point;
  });
  return primaryCaret;
}

function measureSelectionHead(layout, byteInLine) {
  const displayOffset = caretDisplayOffset(layout, byteInLine);
  return measureTextOffset(layout.element, displayOffset);
}

/** Map one source-line byte offset to the currently rendered UTF-16 offset. */
export function caretDisplayOffset(layout, byteInLine) {
  if (layout.isSourceVisible || !layout.isDetailed || !layout.line) {
    return utf8ByteToUtf16(layout.sourceText, byteInLine);
  }
  const lineStart = layout.line.segments[0]?.startByte ?? 0;
  const sourceByte = lineStart + byteInLine;
  const displayByte = sourceByteToDisplayByte(layout.line, sourceByte);
  return utf8ByteToUtf16(layout.line.text, displayByte);
}

function sourceByteToDisplayByte(line, requestedSourceByte) {
  if (line.sourceToDisplay.length > 0) {
    let closest = line.sourceToDisplay[0];
    for (const mapping of line.sourceToDisplay) {
      if (Math.abs(mapping[0] - requestedSourceByte) < Math.abs(closest[0] - requestedSourceByte)) {
        closest = mapping;
      }
      if (mapping[0] === requestedSourceByte) break;
    }
    return closest[1];
  }
  let displayByte = 0;
  for (const segment of line.segments) {
    const displayLength = segment.kind === "visible"
      ? segment.endByte - segment.startByte
      : CARET_ENCODER.encode(segment.replacement).byteLength;
    if (requestedSourceByte <= segment.endByte) {
      if (segment.kind === "visible") {
        return displayByte + Math.max(0, requestedSourceByte - segment.startByte);
      }
      return requestedSourceByte === segment.endByte ? displayByte + displayLength : displayByte;
    }
    displayByte += displayLength;
  }
  return displayByte;
}

/** Measure one rendered UTF-16 insertion point in client coordinates. */
export function measureTextOffset(element, requestedOffset) {
  const textNodes = collectTextNodes(element);
  const textLength = textNodes.reduce((length, node) => length + node.length, 0);
  const offset = Math.max(0, Math.min(requestedOffset, textLength));
  const following = textBoundary(textNodes, offset, true);
  if (following && following.offset < following.node.length) {
    const width = codePointWidth(following.node.nodeValue ?? "", following.offset);
    const bounds = rangeBounds(following.node, following.offset, following.offset + width);
    if (bounds) return { left: bounds.left, top: bounds.top, height: bounds.height };
  }
  const preceding = textBoundary(textNodes, offset, false);
  if (preceding && preceding.offset > 0) {
    const start = previousCodePointStart(preceding.node.nodeValue ?? "", preceding.offset);
    const bounds = rangeBounds(preceding.node, start, preceding.offset);
    if (bounds) return { left: bounds.right, top: bounds.top, height: bounds.height };
  }
  const bounds = element.getBoundingClientRect();
  const lineHeight = parseFloat(getComputedStyle(element).lineHeight) || bounds.height;
  return { left: bounds.left, top: bounds.top, height: lineHeight };
}

/** Measure a rendered UTF-16 range across nested inline projection spans. */
export function measureTextRange(element, requestedStart, requestedEnd) {
  const textNodes = collectTextNodes(element);
  if (textNodes.length === 0 || requestedStart >= requestedEnd) return [];
  const textLength = textNodes.reduce((length, node) => length + node.length, 0);
  const start = Math.max(0, Math.min(requestedStart, textLength));
  const end = Math.max(start, Math.min(requestedEnd, textLength));
  if (start === end) return [];
  const startBoundary = textBoundary(textNodes, start, true);
  const endBoundary = textBoundary(textNodes, end, false);
  if (!startBoundary || !endBoundary) return [];
  const range = document.createRange();
  range.setStart(startBoundary.node, startBoundary.offset);
  range.setEnd(endBoundary.node, endBoundary.offset);
  return [...range.getClientRects()].filter((bounds) => bounds.height > 0);
}

function collectTextNodes(element) {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  const nodes = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) nodes.push(node);
  return nodes;
}

function textBoundary(nodes, requestedOffset, shouldPreferFollowing) {
  let consumed = 0;
  for (const node of nodes) {
    const end = consumed + node.length;
    if (requestedOffset < end || (requestedOffset === end && !shouldPreferFollowing)) {
      return { node, offset: requestedOffset - consumed };
    }
    consumed = end;
  }
  const node = nodes.at(-1);
  return node ? { node, offset: node.length } : null;
}

function rangeBounds(node, start, end) {
  const range = document.createRange();
  range.setStart(node, start);
  range.setEnd(node, end);
  return [...range.getClientRects()].find((bounds) => bounds.height > 0) ?? null;
}

function codePointWidth(text, offset) {
  return text.codePointAt(offset) > 0xFFFF ? 2 : 1;
}

function previousCodePointStart(text, offset) {
  const previous = text.charCodeAt(offset - 1);
  return previous >= 0xDC00 && previous <= 0xDFFF ? Math.max(0, offset - 2) : offset - 1;
}

function isCollapsed(selection) {
  return selection.anchor.line === selection.head.line
    && selection.anchor.byteInLine === selection.head.byteInLine;
}
