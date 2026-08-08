import { projectionDetailedRange } from "/node_modules/@continuity-editor/editor/src/projection.js";

// The visible source-line window.
//
// A host painting scroll-linked chrome over the editor - a sticky heading
// trail, an outline marker - needs to know which source line the reader is
// looking at, and cannot derive it: pixels do not convert to lines outside the
// projection, which measures headings at up to 1.45em and wrapped rows against
// its own font metrics.
//
// The assertions below are geometric rather than numeric, because the exact
// line index depends on the runner's font. Each one states an invariant that
// distinguishes a correct answer from the plausible wrong ones: the realized
// window (two viewports wider in each direction), the first *fully* visible
// line, and a visual row index instead of a source line.

export async function runViewportLineTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const editor = new ContinuityEditorElement();
  // Alternating short and wrapping lines: the short ones establish what a
  // single visual row measures, the long ones give the wrapped-line case
  // something to straddle, and 160 of them put the tail far below the box.
  editor.value = Array.from({ length: 160 }, (_, index) => (index % 2 === 0
    ? `short ${index}`
    : `line ${index} ${"wrapping words that carry this line well past one row ".repeat(3)}`
  )).join("\n");
  editor.setAttribute("aria-label", "Viewport window document");
  editor.style.cssText = "display:block;width:420px;height:240px";

  const seen = [];
  editor.addEventListener("continuity-viewport", (event) => seen.push(event.detail));
  document.body.append(editor);
  await editor.ready;
  await settle();

  const projection = editor.shadowRoot.querySelector(".projection");
  const input = editor.shadowRoot.querySelector("textarea");

  // --- Seeded once, before any movement. ---
  check(seen.length >= 1, `the opening window is published after ready (${seen.length})`);
  assertions += 1;
  check(seen[0]?.version === 1 && seen[0]?.firstLine === 0,
    `the opening window starts at line 0 (${JSON.stringify(seen[0])})`); assertions += 1;
  const opening = editor.visibleLineRange();
  check(opening?.startLine === seen[0].firstLine && opening?.endLine === seen[0].lastLine,
    `the getter agrees with the event (${JSON.stringify(opening)})`); assertions += 1;
  check(opening.endLine > opening.startLine && opening.endLine < 40,
    `the opening window covers the box, not the document (${JSON.stringify(opening)})`);
  assertions += 1;

  // --- Scrolled well into the document. ---
  seen.length = 0;
  input.scrollTop = Math.round(input.scrollHeight / 2);
  input.dispatchEvent(new Event("scroll"));
  await settle();
  check(seen.length >= 1, "scrolling publishes a new window"); assertions += 1;
  const scrolled = editor.visibleLineRange();
  check(scrolled.startLine > opening.startLine,
    `the window followed the scroll (${opening.startLine} -> ${scrolled.startLine})`); assertions += 1;
  assertions += checkWindowEdges(projection, input, scrolled, check, "scrolled");

  // The distinguishing assertion: the projection realizes two extra viewports
  // in each direction, so a host handed the realized window would name a line
  // roughly two screens above the reader.
  const realized = projectionDetailedRange(projection);
  check(scrolled.startLine > realized.start && scrolled.endLine < realized.end - 1,
    `the visible window is strictly inside the realized one (visible ${scrolled.startLine}..${scrolled.endLine}, realized ${realized.start}..${realized.end})`);
  assertions += 1;

  // --- A wrapped line resolves to its own source line. ---
  // Park the top edge inside a line that occupies several rows, so the edge
  // sits in a continuation row. A visual-row answer would name a later line.
  const rowHeight = singleRowHeight(projection);
  const didAlign = alignTopEdgeInsideALine(projection, input, rowHeight);
  input.dispatchEvent(new Event("scroll"));
  await settle();
  check(didAlign, "the top edge was parked inside a multi-row line"); assertions += 1;
  const wrapped = editor.visibleLineRange();
  const wrappedElement = projection.children[wrapped.startLine];
  const wrappedRect = wrappedElement.getBoundingClientRect();
  const wrappedViewport = input.getBoundingClientRect();
  check(wrappedRect.height > rowHeight * 1.5,
    `the reported line occupies several visual rows (${wrappedRect.height} vs row ${rowHeight})`);
  assertions += 1;
  check(wrappedRect.top < wrappedViewport.top && wrappedRect.bottom > wrappedViewport.top,
    "the top edge falls inside that line's continuation rows, not at its boundary");
  assertions += 1;
  check(wrappedElement.dataset.line === String(wrapped.startLine),
    "the reported index addresses that source line's own element"); assertions += 1;
  assertions += checkWindowEdges(projection, input, wrapped, check, "straddling");

  // --- Coalesced to one publication per frame. ---
  seen.length = 0;
  input.scrollTop += 600;
  for (let repeat = 0; repeat < 6; repeat += 1) input.dispatchEvent(new Event("scroll"));
  await settle();
  check(seen.length === 1, `six scroll events in one frame publish once (${seen.length})`);
  assertions += 1;

  // --- An unchanged window is not republished. ---
  seen.length = 0;
  input.dispatchEvent(new Event("scroll"));
  await settle();
  check(seen.length === 0, `a scroll that does not move the window is silent (${seen.length})`);
  assertions += 1;

  // --- Reflow under a stationary scroll offset. ---
  // Inserting whole lines above the viewport shifts every line below without
  // touching scrollTop, so the same pixels now show different source lines.
  seen.length = 0;
  const before = editor.visibleLineRange();
  const stationaryScrollTop = input.scrollTop;
  editor.value = `inserted\ninserted\ninserted\n${editor.value}`;
  await settle();
  await settle();
  check(input.scrollTop === stationaryScrollTop,
    `the host replacement left the scroll offset alone (${stationaryScrollTop} -> ${input.scrollTop})`);
  assertions += 1;
  check(seen.length >= 1, "content reflow under a stationary offset publishes"); assertions += 1;
  check(editor.visibleLineRange().startLine !== before.startLine,
    `the window followed the reflow (${before.startLine} -> ${editor.visibleLineRange().startLine})`);
  assertions += 1;

  // --- Resize. ---
  seen.length = 0;
  const beforeResize = editor.visibleLineRange();
  editor.style.height = "120px";
  await settle();
  await settle();
  check(seen.length >= 1, "a resize publishes"); assertions += 1;
  check(editor.visibleLineRange().endLine < beforeResize.endLine,
    `a shorter box shows fewer lines (${beforeResize.endLine} -> ${editor.visibleLineRange().endLine})`);
  assertions += 1;

  // --- Nothing to measure. ---
  editor.style.display = "none";
  await settle();
  check(editor.visibleLineRange() === null,
    "a hidden editor reports null rather than a window of zeros"); assertions += 1;
  editor.style.display = "block";

  editor.destroy();
  editor.remove();
  return assertions;
}

/**
 * The edge invariants, which are what "visible" means: the reported first line
 * reaches past the top edge and the one before it does not, and the reported
 * last line starts before the bottom edge while the one after it does not.
 */
function checkWindowEdges(projection, scroller, range, check, label) {
  const viewport = scroller.getBoundingClientRect();
  const first = projection.children[range.startLine].getBoundingClientRect();
  const beforeFirst = projection.children[range.startLine - 1]?.getBoundingClientRect();
  const last = projection.children[range.endLine].getBoundingClientRect();
  const afterLast = projection.children[range.endLine + 1]?.getBoundingClientRect();
  check(first.bottom > viewport.top,
    `${label}: the first line has pixels below the top edge`);
  check(!beforeFirst || beforeFirst.bottom <= viewport.top,
    `${label}: the line before it is entirely above the top edge`);
  check(last.top < viewport.bottom,
    `${label}: the last line starts before the bottom edge`);
  check(!afterLast || afterLast.top >= viewport.bottom,
    `${label}: the line after it starts below the bottom edge`);
  return 4;
}

/** Shortest rendered line height, which is one visual row. */
function singleRowHeight(projection) {
  let shortest = Infinity;
  for (let index = 0; index < Math.min(40, projection.children.length); index += 1) {
    shortest = Math.min(shortest, projection.children[index].getBoundingClientRect().height);
  }
  return shortest;
}

/**
 * Scroll until the top edge falls strictly inside a line taller than one row.
 * Reports whether it found one, so the wrapped-line assertions fail loudly
 * rather than passing vacuously against a single-row line.
 *
 * Two steps, because the projection's offset origin and the scroller's scroll
 * origin are not required to coincide - which is half of why a host cannot do
 * this arithmetic from outside. Scroll roughly, measure where the target
 * actually landed, then close the gap by that measured delta.
 */
function alignTopEdgeInsideALine(projection, scroller, rowHeight) {
  scroller.scrollTop = Math.round(scroller.scrollHeight / 3);
  const viewport = scroller.getBoundingClientRect();
  for (let index = 0; index < projection.children.length; index += 1) {
    const rect = projection.children[index].getBoundingClientRect();
    if (rect.bottom <= viewport.top || rect.height <= rowHeight * 1.5) continue;
    scroller.scrollTop += rect.top - viewport.top + Math.floor(rect.height / 2);
    return true;
  }
  return false;
}

function settle() {
  return new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(() => setTimeout(resolve, 0)),
  ));
}
