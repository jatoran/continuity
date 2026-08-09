import { measureTextareaCaretTop } from "/node_modules/@continuity-editor/editor/src/projection_measure.js";
import { animationFrames, settle } from "./browser-touch-helpers.mjs";

/** Exercise host-driven navigation against rendered projection geometry. */
export async function runRangeRevealTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", "Projected range reveal document");
  editor.style.cssText = [
    "display:block",
    "width:310px",
    "height:170px",
    "--continuity-font-family:Georgia,serif",
    "--continuity-font-size:18px",
  ].join(";");
  const lines = buildFixture();
  editor.value = lines.join("\n");
  document.querySelector("#mount").append(editor);
  await editor.ready;
  await settle();

  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  const projection = shadow.querySelector(".projection");
  const shield = shadow.querySelector(".touch-shield");
  const scroller = matchMedia("(pointer: coarse)").matches ? shield : input;
  const scrollOwner = scroller === shield ? "touch shield" : "textarea";
  const lineHeight = Number.parseFloat(getComputedStyle(input).lineHeight);
  const savedSelection = [input.selectionStart, input.selectionEnd, input.selectionDirection];
  input.setSelectionRange(input.value.length, input.value.length);
  const textareaContentHeight = measureTextareaCaretTop(input) + lineHeight;
  input.setSelectionRange(...savedSelection);
  const firstHeading = projection.querySelector('.block-heading-1[data-source-visible="false"]');
  check(
    Number.parseFloat(getComputedStyle(firstHeading).fontSize)
      > Number.parseFloat(getComputedStyle(input).fontSize)
      && Math.abs(projection.offsetHeight - textareaContentHeight) > lineHeight,
    `${scrollOwner} fixture has heading-driven projection/textarea height divergence `
      + `(${projection.offsetHeight}px vs ${Math.round(textareaContentHeight)}px)`,
  );
  assertions += 1;

  const before = editor.snapshot();
  const historyBefore = JSON.stringify(editor.exportHistory());
  const beginning = rangeFor(lines, 1, "opening target");
  const lateWrapped = rangeFor(lines, 42, "LATE-WRAPPED-TARGET");
  const middle = rangeFor(lines, 65, "CENTER-TARGET");
  const ending = rangeFor(lines, lines.length - 1, "ending target");

  await revealAndPaint(editor, ending, "nearest");
  assertions += assertFullyVisible(check, shadow, scroller, `${scrollOwner} nearest end jump`);

  await revealAndPaint(editor, beginning, "nearest");
  assertions += assertFullyVisible(check, shadow, scroller, `${scrollOwner} backward beginning jump`);

  await revealAndPaint(editor, lateWrapped, "nearest");
  assertions += assertFullyVisible(
    check, shadow, scroller, `${scrollOwner} late wrapped source-line match`,
  );
  assertions += assertEdgeClearance(
    check, shadow, scroller, `${scrollOwner} nearest rendered-row clearance`,
  );

  await revealAndPaint(editor, middle, "center");
  assertions += assertCentered(check, shadow, scroller, `${scrollOwner} centered range`);

  scroller.scrollTop = 0;
  scroller.dispatchEvent(new Event("scroll"));
  await animationFrames(2);
  const hostSelection = { anchor: lateWrapped.start, head: lateWrapped.end, kind: "caret" };
  await setSelectionsAndPaint(editor, [hostSelection]);
  assertions += assertFullyVisible(
    check, shadow, scroller, `${scrollOwner} setSelections reveal shares projected navigation`,
  );

  const after = editor.snapshot();
  check(after.text === before.text && after.revision === before.revision,
    `${scrollOwner} reveals do not mutate text or revision`);
  assertions += 1;
  check(JSON.stringify(editor.exportHistory()) === historyBefore,
    `${scrollOwner} reveals do not mutate undo history`);
  assertions += 1;

  input.focus();
  const typingCaret = { anchor: lateWrapped.end, head: lateWrapped.end, kind: "caret" };
  await setSelectionsWithoutReveal(editor, [typingCaret]);
  scroller.scrollTop = 0;
  scroller.dispatchEvent(new Event("scroll"));
  await animationFrames(2);
  const typingFrame = onceFrame(editor);
  editor.insertText("!");
  await typingFrame;
  await animationFrames(2);
  assertions += assertFullyVisible(
    check, shadow, scroller, `${scrollOwner} typing follows the projected caret`,
  );
  assertions += assertEdgeClearance(
    check, shadow, scroller, `${scrollOwner} typing keeps rendered-row clearance`,
  );

  editor.destroy();
  editor.remove();
  return assertions;
}

function buildFixture() {
  const lines = [];
  for (let index = 0; index < 34; index += 1) {
    lines.push(`# Heading ${index} ${"scaled heading words ".repeat(3)}`);
    lines.push(index === 0
      ? "opening target near the beginning"
      : `body ${index} ${"ordinary body words ".repeat(2)}`);
  }
  lines[42] = `- ${"wrapped payload ".repeat(24)}LATE-WRAPPED-TARGET tail`;
  lines[65] = `middle prose ${"centering context ".repeat(8)}CENTER-TARGET tail`;
  lines.push("ending target near the document boundary");
  return lines;
}

function rangeFor(lines, line, needle) {
  const start = lines[line].indexOf(needle);
  return {
    start: { line, byteInLine: start },
    end: { line, byteInLine: start + needle.length },
  };
}

async function revealAndPaint(editor, range, align) {
  const frame = onceFrame(editor);
  editor.revealRange(range, { align });
  await frame;
  await animationFrames(2);
}

async function setSelectionsAndPaint(editor, selections) {
  const frame = onceFrame(editor);
  editor.setSelections(selections, { reveal: true });
  await frame;
  await animationFrames(2);
}

async function setSelectionsWithoutReveal(editor, selections) {
  const frame = onceFrame(editor);
  editor.setSelections(selections, { reveal: false });
  await frame;
  await animationFrames(2);
}

function onceFrame(editor) {
  return new Promise((resolve) => editor.addEventListener("continuity-frame", resolve, { once: true }));
}

function assertFullyVisible(check, shadow, scroller, label) {
  const target = primaryTargetBounds(shadow);
  const viewport = scroller.getBoundingClientRect();
  check(
    target && target.top >= viewport.top - 1 && target.bottom <= viewport.bottom + 1,
    `${label} is fully visible (${formatBounds(target)} in ${formatBounds(viewport)})`,
  );
  return 1;
}

function assertCentered(check, shadow, scroller, label) {
  const target = primaryTargetBounds(shadow);
  const viewport = scroller.getBoundingClientRect();
  const targetCenter = target ? (target.top + target.bottom) / 2 : Number.NaN;
  const viewportCenter = (viewport.top + viewport.bottom) / 2;
  check(
    target && Math.abs(targetCenter - viewportCenter) <= 2,
    `${label} is centered (${Math.round(targetCenter)} vs ${Math.round(viewportCenter)})`,
  );
  return 1;
}

function assertEdgeClearance(check, shadow, scroller, label) {
  const target = primaryTargetBounds(shadow);
  const viewport = scroller.getBoundingClientRect();
  const clearance = target?.rowHeight ?? 0;
  check(
    target
      && target.top >= viewport.top + clearance - 1
      && target.bottom <= viewport.bottom - clearance + 1,
    `${label} is preserved (${formatBounds(target)} in ${formatBounds(viewport)})`,
  );
  return 1;
}

function primaryTargetBounds(shadow) {
  const elements = [...shadow.querySelectorAll(
    '.visual-selection[data-selection-index="0"], .primary-caret',
  )];
  if (elements.length === 0) return null;
  const rects = elements.map((element) => element.getBoundingClientRect());
  return {
    top: Math.min(...rects.map((rect) => rect.top)),
    bottom: Math.max(...rects.map((rect) => rect.bottom)),
    rowHeight: Math.max(...rects.map((rect) => rect.height)),
  };
}

function formatBounds(bounds) {
  return bounds ? `${Math.round(bounds.top)}..${Math.round(bounds.bottom)}` : "missing";
}
