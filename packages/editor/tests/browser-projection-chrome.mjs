// Projection chrome: thematic breaks, hanging indent, indent guides, host
// decorations, and touch selection handles.
//
// All of these are geometry claims, so they run in a real browser. jsdom
// measures nothing — every assertion here would pass vacuously against it.

import {
  animationFrames,
  glyphRect,
  longPress,
  mountEditor as mountSharedEditor,
  placeCaret,
  pointerEvent,
  settle,
} from "./browser-touch-helpers.mjs";

/** Dispatch the pointer sequence a mouse click produces on the textarea. */
function mouseClick(input, clientX, clientY) {
  const mouse = { pointerType: "mouse", pointerId: 1 };
  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY, mouse));
  input.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { ...mouse, buttons: 0 }));
  input.dispatchEvent(new PointerEvent("click", {
    bubbles: true, cancelable: true, pointerType: "mouse", pointerId: 1,
    button: 0, buttons: 0, clientX, clientY, detail: 1,
  }));
}

const MOUNT_OPTIONS = { label: "Projection chrome document" };

/** Mount one editor for these suites under a distinct accessible name. */
function mountEditor(ContinuityEditorElement, mount, value) {
  return mountSharedEditor(ContinuityEditorElement, mount, value, MOUNT_OPTIONS);
}

export async function runProjectionChromeTests(ContinuityEditorElement, check) {
  const mount = document.querySelector("#mount");
  let assertions = 0;
  assertions += await runThematicBreakCases(ContinuityEditorElement, check, mount);
  assertions += await runHangingIndentCases(ContinuityEditorElement, check, mount);
  assertions += await runIndentGuideCases(ContinuityEditorElement, check, mount);
  assertions += await runDecorationCases(ContinuityEditorElement, check, mount);
  assertions += await runPipeTableCases(ContinuityEditorElement, check, mount);
  return assertions;
}

/**
 * Handles are coarse-pointer chrome, so like the shield contract they only mean
 * anything under touch emulation; the runner re-runs this file's touch half
 * with a coarse primary pointer.
 */
export async function runSelectionHandleTests(ContinuityEditorElement, check) {
  if (!matchMedia("(pointer: coarse)").matches) return 0;
  return runSelectionHandleCases(ContinuityEditorElement, check, document.querySelector("#mount"));
}

/**
 * A thematic break projects to an empty display line, so without a rule of its
 * own it renders as a blank line. The raw source has to come back under the
 * caret, the way heading scaling does.
 */
async function runThematicBreakCases(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount, "before the break\n\n---\n\nafter the break\n",
  );
  let count = 0;
  placeCaret(input, 0);
  await settle();

  const line = projection.children[2];
  check(
    line.className.includes("block-horizontalRule"),
    `the engine classifies the break line (got ${JSON.stringify(line.className)})`,
  );
  count += 1;
  check(
    line.dataset.sourceVisible === "false" && line.textContent === "",
    `an unfocused break folds its dashes away (visible ${line.dataset.sourceVisible}, text ${JSON.stringify(line.textContent)})`,
  );
  count += 1;

  const rule = getComputedStyle(line, "::after");
  check(
    rule.content === '""' && rule.height !== "0px" && rule.height !== "auto",
    `a folded break paints a rule (content ${rule.content}, height ${rule.height})`,
  );
  count += 1;
  check(
    rule.backgroundColor !== "rgba(0, 0, 0, 0)",
    `the rule takes the border colour (got ${rule.backgroundColor})`,
  );
  count += 1;

  // The caret's own line reveals raw source, and the rule must step aside.
  placeCaret(input, editor.value.indexOf("---") + 1);
  await settle();
  const focused = projection.children[2];
  check(
    focused.dataset.sourceVisible === "true" && focused.textContent === "---",
    `the caret's break line shows raw source (got ${JSON.stringify(focused.textContent)})`,
  );
  count += 1;
  check(
    getComputedStyle(focused, "::after").content === "none",
    "no rule is drawn over the revealed source",
  );
  count += 1;

  dispose();
  return count;
}

/**
 * A pipe table projects as one grid row per source line sharing a column
 * template: the engine hides the pipes, the cells are the text between them,
 * the delimiter row collapses, and the caret's own row comes back as raw source.
 */
async function runPipeTableCases(ContinuityEditorElement, check, mount) {
  const value = [
    "| # | NAME | EMAIL |",
    "| --- | :---: | ---: |",
    "| 1 | Carolla's best buddy | [Send Email](https://example.com/mail?id=1) |",
    "| 2 | Bend the Knee | plain |",
    "after",
    "",
  ].join("\n");
  const { editor, input, projection, dispose } = await mountEditor(ContinuityEditorElement, mount, value);
  let count = 0;
  placeCaret(input, value.indexOf("after") + 1);
  await settle();

  const rows = [0, 1, 2, 3].map((index) => projection.children[index]);
  check(
    rows.every((row) => row.classList.contains("table-row")),
    `every table line renders as a grid row (${rows.map((row) => row.className).join(" / ")})`,
  );
  count += 1;
  check(
    rows[0].classList.contains("table-header") && rows[1].classList.contains("table-delimiter"),
    "the first row is the header and the second is the collapsed delimiter",
  );
  count += 1;
  check(
    getComputedStyle(rows[0]).display === "grid" && rows[0].querySelectorAll(".table-cell").length === 3,
    `the header lays out as a three-column grid (display ${getComputedStyle(rows[0]).display}, cells ${rows[0].querySelectorAll(".table-cell").length})`,
  );
  count += 1;
  check(
    !rows[0].textContent.includes("|") && !rows[2].textContent.includes("|"),
    "pipes are hidden in projected rows",
  );
  count += 1;
  check(
    rows[1].getBoundingClientRect().height === 0 && rows[1].textContent === "",
    `the delimiter row takes no height (got ${rows[1].getBoundingClientRect().height})`,
  );
  count += 1;
  const headerCells = [...rows[0].querySelectorAll(".table-cell")];
  const bodyCells = [...rows[2].querySelectorAll(".table-cell")];
  const columnsAlign = headerCells.every((cell, column) => (
    Math.abs(cell.getBoundingClientRect().left - bodyCells[column].getBoundingClientRect().left) <= 0.75
  ));
  check(columnsAlign, "header and body columns share the same left edges");
  count += 1;
  check(
    bodyCells[1].classList.contains("table-align-center") && bodyCells[2].classList.contains("table-align-right"),
    "delimiter alignments reach the body cells",
  );
  count += 1;
  const link = bodyCells[2].querySelector(".inline-link");
  check(
    link?.textContent === "Send Email" && !bodyCells[2].textContent.includes("https://"),
    `a link in a cell shows its text only (got ${JSON.stringify(bodyCells[2].textContent)})`,
  );
  count += 1;
  const widest = headerCells[1].getBoundingClientRect().width;
  check(
    widest > headerCells[0].getBoundingClientRect().width,
    "the NAME column is wider than the # column because its widest cell is wider",
  );
  count += 1;

  // Tapping inside a projected cell lands the caret on that source byte.
  const bendRect = glyphRect(bodyCells[1], 1);
  const bendCell = [...rows[3].querySelectorAll(".table-cell")][1];
  const bendGlyph = glyphRect(bendCell, 1);
  mouseClick(input, bendGlyph.left + 1, bendGlyph.top + bendGlyph.height / 2);
  await settle();
  const caretLine = value.slice(0, input.selectionStart).split("\n").length - 1;
  check(
    caretLine === 3 && value.slice(input.selectionStart, input.selectionStart + 4) === "Bend",
    `clicking a cell glyph places the caret on that byte (line ${caretLine}, before ${JSON.stringify(value.slice(input.selectionStart, input.selectionStart + 4))})`,
  );
  count += 1;
  check(
    rows[3].dataset.sourceVisible === "true"
      && rows[3].textContent === "| 2 | Bend the Knee | plain |"
      && getComputedStyle(rows[3]).display !== "grid",
    `the caret's row reveals raw source (got ${JSON.stringify(rows[3].textContent)})`,
  );
  count += 1;
  check(
    rows[2].classList.contains("table-row") && Math.abs(bendRect.left - glyphRect(bodyCells[1], 1).left) <= 0.75,
    "the other rows keep their grid and column positions while one row is being edited",
  );
  count += 1;

  dispose();
  return count;
}

/**
 * Wrapped rows hang beneath the content the first row starts with. Tab-indented
 * lines used to disagree: expressing the hanging indent as inline padding plus a
 * negative first-line indent anchors the tab-stop grid at the padded content
 * edge while pulling the first row left of it, so a nested bullet's own text
 * rendered at `indent mod tab-width` while its wrapped rows hung at `indent`.
 * Space-indented lines were unaffected, which is why only nested lines looked
 * wrong. Checked under the default typography and under a host-supplied
 * proportional font, because the error scales with the font's space advance.
 */
async function runHangingIndentCases(ContinuityEditorElement, check, mount) {
  const filler = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount, [
      `- top level bullet ${filler}`,
      `\t- nested bullet ${filler}`,
      `\t\t- twice nested bullet ${filler}`,
      `    - space nested bullet ${filler}`,
      "tail",
    ].join("\n"),
  );
  editor.style.width = "420px";
  let count = 0;

  for (const typography of ["default", "proportional"]) {
    if (typography === "proportional") {
      editor.style.setProperty("--continuity-font-family", '"Segoe UI", system-ui, sans-serif');
      editor.style.setProperty("--continuity-font-size", "15px");
    }
    placeCaret(input, editor.value.length);
    await settle();

    for (const lineIndex of [0, 1, 2, 3]) {
      const element = projection.children[lineIndex];
      const rendered = element.textContent;
      const contentStart = rendered.search(/[A-Za-z]/u);
      const rows = rowStarts(element);
      const firstContent = glyphRect(element, contentStart);
      check(rows.length > 1, `[${typography}] line ${lineIndex} wraps`);
      count += 1;
      check(
        Boolean(firstContent) && Math.abs(rows[1] - firstContent.left) <= 1.5,
        `[${typography}] line ${lineIndex} wrapped rows hang under its own content `
        + `(row 1 at ${round(rows[1])}, content starts at ${round(firstContent?.left)})`,
      );
      count += 1;
    }
  }

  editor.style.removeProperty("--continuity-font-family");
  editor.style.removeProperty("--continuity-font-size");
  dispose();
  return count;
}

/** Indent guides are opt-in chrome keyed off the projection's own flag. */
async function runIndentGuideCases(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount, [
      "- top",
      "\t- nested",
      "\t\t- twice nested",
      "",
      "\t\t- after a blank line",
      "tail",
    ].join("\n"),
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  check(
    projection.dataset.indentGuides === "off",
    `guides default to off (got ${projection.dataset.indentGuides})`,
  );
  count += 1;
  check(
    getComputedStyle(projection.children[2]).backgroundImage === "none",
    "no guide is painted while guides are off",
  );
  count += 1;

  editor.indentGuides = "on";
  await settle();
  check(
    projection.dataset.indentGuides === "on" && editor.getAttribute("indent-guides") === "on",
    "the property reflects to the attribute and the projection",
  );
  count += 1;

  // The authored custom property keeps `var()` intact, so column structure is
  // readable from it; the computed background only proves something painted.
  const guideColumns = (index) => {
    const value = projection.children[index].style.getPropertyValue("--continuity-line-guides");
    return value.match(/--continuity-indent-guide[^)]*\) (-?[\d.]+)px/gu) ?? [];
  };

  // Depth 0 draws nothing: a guide marks an *enclosing* parent, and a
  // top-level line has none.
  check(
    getComputedStyle(projection.children[0]).backgroundImage === "none",
    `a top-level line draws no guide (got ${getComputedStyle(projection.children[0]).backgroundImage})`,
  );
  count += 1;
  // Depth 1's only parent is the body's left edge, and it is drawn: a page of
  // singly-indented content has to read as indented while scrolling past it.
  check(
    guideColumns(1).length === 1 && guideColumns(1)[0].endsWith(" 0px"),
    `a single-level line draws the left-edge column at 0px (got ${guideColumns(1).join(", ")})`,
  );
  count += 1;
  check(
    guideColumns(2).length === 2 && guideColumns(2)[0].endsWith(" 0px"),
    `a twice-nested line draws the left edge and its parent (got ${guideColumns(2).join(", ")})`,
  );
  count += 1;
  check(
    getComputedStyle(projection.children[3]).backgroundImage.includes("gradient"),
    "a blank line inherits the guides its neighbours share",
  );
  count += 1;
  check(
    getComputedStyle(projection.children[5]).backgroundImage === "none",
    "a top-level line after nested content draws no guide",
  );
  count += 1;

  editor.indentGuides = "off";
  await settle();
  check(
    getComputedStyle(projection.children[2]).backgroundImage === "none",
    "turning guides off stops painting them",
  );
  count += 1;

  dispose();
  return count;
}

/** Host decorations paint ranges without touching selection or history. */
async function runDecorationCases(ContinuityEditorElement, check, mount) {
  const { editor, input, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "alpha needle beta\nsecond needle line\nthird line without\n",
  );
  let count = 0;
  placeCaret(input, 0);
  await settle();

  const revisionBefore = editor.snapshot().revision;
  const selectionBefore = JSON.stringify(editor.snapshot().selections);
  const painted = editor.setDecorations("find-match", [
    { start: { line: 0, byteInLine: 6 }, end: { line: 0, byteInLine: 12 } },
    { start: { line: 1, byteInLine: 7 }, end: { line: 1, byteInLine: 13 } },
  ]);
  await settle();

  check(painted === 2, `both ranges are retained (got ${painted})`);
  count += 1;
  const rects = editor.shadowRoot.querySelectorAll(".decoration");
  check(rects.length === 2, `each range paints one rectangle (got ${rects.length})`);
  count += 1;
  check(
    editor.snapshot().revision === revisionBefore
      && JSON.stringify(editor.snapshot().selections) === selectionBefore,
    "decorating changes neither revision nor selection",
  );
  count += 1;
  check(
    rects[0]?.getAttribute("part") === "decoration decoration-find-match",
    `each rectangle exposes a per-set part (got ${rects[0]?.getAttribute("part")})`,
  );
  count += 1;

  // The rectangle must sit on the glyphs it names, not at the line's origin.
  const projection = editor.shadowRoot.querySelector(".projection");
  const needle = glyphRect(projection.children[0], projection.children[0].textContent.indexOf("needle"));
  const rectBounds = rects[0].getBoundingClientRect();
  check(
    Math.abs(rectBounds.left - needle.left) <= 1.5 && Math.abs(rectBounds.top - needle.top) <= 1.5,
    `the rectangle covers the decorated glyphs (rect ${round(rectBounds.left)},${round(rectBounds.top)} vs glyph ${round(needle.left)},${round(needle.top)})`,
  );
  count += 1;

  editor.style.setProperty("--continuity-decoration-find-match", "rgb(10, 200, 30)");
  await settle();
  check(
    getComputedStyle(editor.shadowRoot.querySelector(".decoration")).backgroundColor
      === "rgb(10, 200, 30)",
    "a per-set custom property themes only that set",
  );
  count += 1;

  editor.setDecorations("find-match", []);
  await settle();
  check(
    editor.shadowRoot.querySelectorAll(".decoration").length === 0,
    "an empty range list removes the set",
  );
  count += 1;

  editor.setDecorations("active-match", [
    { start: { line: 0, byteInLine: 0 }, end: { line: 0, byteInLine: 5 } },
  ]);
  await settle();
  editor.clearDecorations();
  await settle();
  check(
    editor.shadowRoot.querySelectorAll(".decoration").length === 0,
    "clearDecorations with no id removes every set",
  );
  count += 1;

  let rejected = false;
  try {
    editor.setDecorations("not a css ident", []);
  } catch {
    rejected = true;
  }
  check(rejected, "an id that could not name a custom property is refused");
  count += 1;

  dispose();
  return count;
}

/**
 * The touch shield displaces the platform's own drag handles, so a selection
 * drawn by a long-press could not be adjusted afterwards. Also pins the
 * behaviour of the post-long-press guard against deliberate host selections:
 * the guard exists to reject the platform's competing answer, not the host's.
 */
async function runSelectionHandleCases(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, shield, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "alpha beta gamma delta epsilon zeta eta theta\nsecond line of prose here\n",
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  const element = projection.children[0];
  const rendered = element.textContent;
  const target = glyphRect(element, rendered.indexOf("gamma") + 2);
  const clientX = target.left + target.width / 2;
  const clientY = target.top + target.height / 2;

  // Under a coarse pointer the finger lands on the shield, never the textarea.
  shield.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  await longPress();
  shield.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));
  await settle();

  check(
    input.value.slice(input.selectionStart, input.selectionEnd) === "gamma",
    `the long-press left a word selected (got ${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd))})`,
  );
  count += 1;

  const handles = [...editor.shadowRoot.querySelectorAll(".selection-handle")];
  const visible = handles.filter((handle) => !handle.hidden);
  check(visible.length === 2, `both handles are offered once the finger lifts (got ${visible.length})`);
  count += 1;

  const endHandle = handles.find((handle) => handle.dataset.selectionEdge === "end");
  const handleBounds = endHandle.getBoundingClientRect();
  const selectionRects = editor.shadowRoot.querySelectorAll(".visual-selection");
  const selectionEnd = selectionRects[selectionRects.length - 1].getBoundingClientRect();
  check(
    Math.abs((handleBounds.left + handleBounds.width / 2) - selectionEnd.right) <= 2,
    `the end handle is centred on the selection's end (handle ${round(handleBounds.left + handleBounds.width / 2)} vs selection ${round(selectionEnd.right)})`,
  );
  count += 1;

  // Drag the end handle forward: the selection must grow, not restart.
  const grabX = handleBounds.left + handleBounds.width / 2;
  const grabY = handleBounds.top + handleBounds.height / 2;
  const destination = glyphRect(element, rendered.indexOf("epsilon") + 7);
  endHandle.dispatchEvent(pointerEvent("pointerdown", grabX, grabY, { pointerId: 31 }));
  endHandle.dispatchEvent(pointerEvent(
    "pointermove",
    grabX + (destination.left + destination.width - selectionEnd.right),
    grabY + (destination.top - selectionEnd.top),
    { pointerId: 31 },
  ));
  await animationFrames(3);
  const grown = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    grown.startsWith("gamma") && grown.includes("epsilon"),
    `dragging the end handle extends the selection (got ${JSON.stringify(grown)})`,
  );
  count += 1;

  endHandle.dispatchEvent(pointerEvent("pointerup", grabX, grabY, { pointerId: 31, buttons: 0 }));
  await settle();

  // Drag the start handle backwards, pivoting on the frozen far end.
  const startHandle = handles.find((handle) => handle.dataset.selectionEdge === "start");
  const startBounds = startHandle.getBoundingClientRect();
  const startGrabX = startBounds.left + startBounds.width / 2;
  const startGrabY = startBounds.top + startBounds.height / 2;
  const back = glyphRect(element, rendered.indexOf("alpha"));
  startHandle.dispatchEvent(pointerEvent("pointerdown", startGrabX, startGrabY, { pointerId: 32 }));
  startHandle.dispatchEvent(pointerEvent(
    "pointermove", startGrabX - (target.left - back.left) - 40, startGrabY, { pointerId: 32 },
  ));
  await animationFrames(3);
  const widened = input.value.slice(input.selectionStart, input.selectionEnd);
  startHandle.dispatchEvent(pointerEvent("pointerup", startGrabX, startGrabY, { pointerId: 32, buttons: 0 }));
  check(
    widened.startsWith("alpha") && widened.includes("epsilon"),
    `dragging the start handle back keeps the far end fixed (got ${JSON.stringify(widened)})`,
  );
  count += 1;

  // A host selection taken immediately after a long-press must land: the
  // platform-guard window only refuses adopting the platform's own answer.
  shield.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  await longPress();
  shield.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));
  editor.setSelections([{
    anchor: { line: 1, byteInLine: 0 }, head: { line: 1, byteInLine: 6 }, kind: "caret",
  }]);
  await settle();
  const hostSelected = editor.snapshot().selections[0];
  check(
    hostSelected.head.line === 1 && hostSelected.head.byteInLine === 6,
    `a host selection inside the platform-guard window is applied (got ${hostSelected.head.line}:${hostSelected.head.byteInLine})`,
  );
  count += 1;

  dispose();
  return count;
}

// --- helpers -------------------------------------------------------------

/** Left edge of each rendered row of one line element. */
function rowStarts(element) {
  const range = document.createRange();
  range.selectNodeContents(element);
  const rows = [];
  for (const rect of range.getClientRects()) {
    if (rect.height <= 0) continue;
    const existing = rows.find((row) => Math.abs(row.top - rect.top) <= 1);
    if (existing) existing.left = Math.min(existing.left, rect.left);
    else rows.push({ top: rect.top, left: rect.left });
  }
  return rows.sort((left, right) => left.top - right.top).map((row) => row.left);
}

function round(value) {
  return value === undefined ? "n/a" : Math.round(value * 10) / 10;
}
