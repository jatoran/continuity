// Touch selection against projected geometry.
//
// The editor is an overlay: an invisible textarea holds raw source and receives
// input, while `.projection` renders the Markdown the reader actually sees. The
// two layouts diverge by construction — folded markers shorten projected text,
// headings render up to 1.45em, the projection box is 14px narrower, and list
// lines carry a wrap indent the textarea has no equivalent for. Every assertion
// here pins behaviour to the projection, because that is the only geometry the
// user can see.
//
// These run in a real browser (see `browser-runner.mjs`); jsdom cannot measure
// `getClientRects`, wrapping, or heading scaling, and would pass vacuously.
import {
  animationFrames,
  glyphRect,
  longPress,
  mountEditor,
  placeCaret,
  pointerEvent,
  projectedCharToSourceByte,
  settle,
  suppressSelectEvent,
  tap,
  utf16IndexForByte,
} from "./browser-touch-helpers.mjs";

export async function runTouchSelectionTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const mount = document.querySelector("#mount");

  for (const theme of ["dark", "light"]) {
    assertions += await runProjectedTapCases(ContinuityEditorElement, check, mount, theme);
  }
  assertions += await runCaretSyncCase(ContinuityEditorElement, check, mount);
  assertions += await runLongPressCases(ContinuityEditorElement, check, mount);
  assertions += await runCompositionTapCase(ContinuityEditorElement, check, mount);
  assertions += await runStaleCaretCases(ContinuityEditorElement, check, mount);
  assertions += await runDragStabilityCase(ContinuityEditorElement, check, mount);
  assertions += await runComposingGestureCases(ContinuityEditorElement, check, mount);
  assertions += await runWordAnchorCases(ContinuityEditorElement, check, mount);
  return assertions;
}

/**
 * A long-press lands the finger in the *middle* of a word. Anchoring at one end
 * and letting the head follow the finger collapses the selection to the part of
 * the word between them: the highlight sits beside the finger instead of under
 * it, and cannot grow until the finger passes the far edge.
 */
async function runWordAnchorCases(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "alpha beta gamma delta epsilon zeta eta theta\nsecond line of prose here\n",
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  const element = projection.children[0];
  const rendered = element.textContent;
  const wordStart = rendered.indexOf("gamma");
  // Press in the middle of "gamma", which is where a finger actually lands.
  const middle = glyphRect(element, wordStart + 2);
  const clientX = middle.left + middle.width / 2;
  const clientY = middle.top + middle.height / 2;

  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  await longPress();
  check(
    input.value.slice(input.selectionStart, input.selectionEnd) === "gamma",
    `long-press mid-word selects the whole word (got ${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd))})`,
  );
  count += 1;

  // A move that does not leave the word must not shrink it.
  input.dispatchEvent(pointerEvent("pointermove", clientX, clientY));
  await settle();
  check(
    input.value.slice(input.selectionStart, input.selectionEnd) === "gamma",
    `a move inside the word keeps it whole (got ${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd))})`,
  );
  count += 1;

  // Dragging forward past the word grows from the word's end.
  const forward = glyphRect(element, rendered.indexOf("epsilon") + 7);
  input.dispatchEvent(pointerEvent(
    "pointermove", forward.left + forward.width - 1, forward.top + forward.height / 2,
  ));
  await settle();
  const grown = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    grown.startsWith("gamma") && grown.includes("epsilon"),
    `dragging forward grows from the word's start (got ${JSON.stringify(grown)})`,
  );
  count += 1;

  // Dragging back before the word grows from the word's start.
  const backward = glyphRect(element, rendered.indexOf("alpha"));
  input.dispatchEvent(pointerEvent(
    "pointermove", backward.left + 1, backward.top + backward.height / 2,
  ));
  await settle();
  const backwards = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    backwards.startsWith("alpha") && backwards.endsWith("gamma"),
    `dragging back grows from the word's end (got ${JSON.stringify(backwards)})`,
  );
  count += 1;

  input.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));
  dispose();
  return count;
}

async function runComposingGestureCases(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "# Heading here\nprose with **bold text** and more words after it\ntail line\n",
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  const element = projection.children[1];
  const rendered = element.textContent;
  const rect = glyphRect(element, rendered.indexOf("bold") + 1);
  const clientX = rect.left + rect.width / 2;
  const clientY = rect.top + rect.height / 2;

  // Open a composition and leave it open, as GBoard does.
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  input.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: "bold" }));
  await settle();

  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  await longPress();

  const selected = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    selected === "bold",
    `long-press selects through the projection while a composition is open (got ${JSON.stringify(selected)})`,
  );
  count += 1;

  const selectStart = new Event("selectstart", { bubbles: true, cancelable: true });
  input.dispatchEvent(selectStart);
  check(
    selectStart.defaultPrevented,
    "a long-press during composition still suppresses the platform selection",
  );
  count += 1;

  input.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));
  // Wait out the window during which a claimed gesture refuses the platform's
  // competing selection, so this case exercises ordinary adoption.
  await new Promise((resolve) => setTimeout(resolve, 1000));
  await settle();

  // A platform selection made during a composition must reach the engine, or
  // the drawn highlight is not merely stale but absent: the textarea's own
  // ::selection is transparent.
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  const from = editor.value.indexOf("prose");
  const to = editor.value.indexOf("tail");
  input.setSelectionRange(from, to);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await settle();

  const range = engineRange(editor);
  check(
    Boolean(range) && range[0] === from && range[1] === to,
    `a selection made during composition reaches the engine (engine ${JSON.stringify(range)} vs textarea ${from}..${to})`,
  );
  count += 1;
  check(
    editor.shadowRoot.querySelectorAll(".visual-selection").length > 0,
    "a multi-line selection during composition is still painted",
  );
  count += 1;
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "" }));

  dispose();
  return count;
}

/** The engine's primary selection as an ordered source-offset pair. */
function engineRange(editor) {
  const snapshot = editor.snapshot();
  const selection = snapshot.selections[0];
  if (!selection) return null;
  const lines = snapshot.text.split("\n");
  const toOffset = (position) => {
    let offset = 0;
    for (let index = 0; index < position.line && index < lines.length; index += 1) {
      offset += lines[index].length + 1;
    }
    return offset + position.byteInLine;
  };
  const anchor = toOffset(selection.anchor);
  const head = toOffset(selection.head);
  return [Math.min(anchor, head), Math.max(anchor, head)];
}

/**
 * The drawn caret must never disagree with the textarea about where typing
 * lands. When it does, the platform's own caret marker sits at the tap while
 * the blinking caret stays behind, and only catches up once the user types.
 */
async function runStaleCaretCases(ContinuityEditorElement, check, mount) {
  const { editor, input, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "# Heading\nfirst prose line here\nsecond prose line here\nthird prose line here\n",
  );
  let count = 0;

  // A tap the projection cannot hit-test must still track the platform caret
  // rather than silently leaving the engine behind.
  const bounds = input.getBoundingClientRect();
  editor.setSelections(
    [{ anchor: { line: 0, byteInLine: 0 }, head: { line: 0, byteInLine: 0 }, kind: "caret" }],
    { reveal: false },
  );
  await settle();
  const target = editor.value.indexOf("third");
  input.setSelectionRange(target, target);
  // Far below the last glyph: the platform places a caret, the projection
  // cannot resolve a glyph.
  tap(input, bounds.right - 2, bounds.bottom - 2);
  await settle();

  check(
    engineCaretOffset(editor) === input.selectionStart,
    `an unresolvable tap still tracks the platform caret (engine ${engineCaretOffset(editor)} vs textarea ${input.selectionStart})`,
  );
  count += 1;

  // A caret parked mid-composition must not be left stale while the two
  // buffers still agree.
  editor.setSelections(
    [{ anchor: { line: 0, byteInLine: 0 }, head: { line: 0, byteInLine: 0 }, kind: "caret" }],
    { reveal: false },
  );
  await settle();
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  const second = editor.value.indexOf("second");
  input.setSelectionRange(second, second);
  tap(input, bounds.left + 4, bounds.top + 4);
  await settle();
  check(
    engineCaretOffset(editor) === input.selectionStart,
    `a tap during an idle composition keeps the drawn caret with the platform caret (engine ${engineCaretOffset(editor)} vs textarea ${input.selectionStart})`,
  );
  count += 1;
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "" }));

  dispose();
  return count;
}

/**
 * Glyphs must not move underneath a finger that is dragging a selection.
 * Revealing raw source on the line the selection head enters (and re-folding
 * the one it leaves) re-lays-out the text mid-gesture, which reads as the drag
 * starting in the wrong place and correcting once the finger stops.
 */
async function runDragStabilityCase(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount, [
      "para one with **bold** and `code` markers inside it",
      "para two with **more** and `spans` to cross over",
      "para three with **final** and `bits` at the end",
      "tail",
    ].join("\n"),
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  const first = projection.children[0];
  const start = glyphRect(first, 5);
  const clientX = start.left + start.width / 2;
  const clientY = start.top + start.height / 2;

  const widthsBefore = [0, 1, 2].map((index) => projection.children[index].textContent.length);

  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  await longPress();

  // Drag across two more lines, sampling the rendered text each step.
  const observed = [];
  for (const lineIndex of [1, 2]) {
    const target = glyphRect(projection.children[lineIndex], 8);
    input.dispatchEvent(pointerEvent(
      "pointermove", target.left + target.width / 2, target.top + target.height / 2,
    ));
    await animationFrames(3);
    observed.push([0, 1, 2].map((index) => projection.children[index].textContent.length));
  }

  const stable = observed.every((sample) => sample.every(
    (length, index) => length === widthsBefore[index],
  ));
  check(
    stable,
    `projected glyphs stay put while a selection drag is in flight (${JSON.stringify(widthsBefore)} vs ${JSON.stringify(observed)})`,
  );
  count += 1;

  const selected = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    selected.includes("para two"),
    `the drag selection spans the lines the finger crossed (got ${JSON.stringify(selected.slice(0, 40))})`,
  );
  count += 1;

  input.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));
  dispose();
  return count;
}

/** Source offset of the engine's primary caret. */
function engineCaretOffset(editor) {
  const snapshot = editor.snapshot();
  const head = snapshot.selections[0]?.head;
  if (!head) return null;
  const lines = snapshot.text.split("\n");
  let offset = 0;
  for (let index = 0; index < head.line && index < lines.length; index += 1) {
    offset += lines[index].length + 1;
  }
  return offset + head.byteInLine;
}

/** A tap must land on the source byte under the projected glyph it touched. */
async function runProjectedTapCases(ContinuityEditorElement, check, mount, theme) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount, [
      "# A heading whose glyphs render larger than the raw source line",
      "prose with **bold text** and `inline code` and [a link](https://example.com/x)",
      "- a list item long enough that its wrapped rows hang beneath the bullet indent",
      "plain trailing line",
    ].join("\n"), { theme },
  );

  let count = 0;
  // Line 3 stays the caret line so lines 0-2 remain projected (markers folded,
  // headings scaled) rather than reverting to source-visible raw text.
  placeCaret(input, editor.value.length);
  await settle();

  for (const lineIndex of [0, 1, 2]) {
    const element = projection.children[lineIndex];
    const rendered = element.textContent;
    // Aim at a glyph well inside the line so the divergence has accumulated.
    const charIndex = Math.max(1, Math.floor(rendered.length * 0.6));
    const rect = glyphRect(element, charIndex);
    if (!rect) continue;
    tap(input, rect.left + rect.width * 0.2, rect.top + rect.height / 2);
    await settle();

    const head = editor.snapshot().selections[0].head;
    const sourceLine = editor.value.split("\n")[lineIndex];
    const expectedByte = projectedCharToSourceByte(sourceLine, rendered, charIndex);
    check(
      head.line === lineIndex,
      `[${theme}] tap on projected line ${lineIndex} selects that source line (got ${head.line})`,
    );
    count += 1;
    check(
      Math.abs(head.byteInLine - expectedByte) <= 1,
      `[${theme}] tap under projected glyph ${charIndex} maps to source byte ${expectedByte} (got ${head.byteInLine})`,
    );
    count += 1;

    // The drawn caret must land on the glyph its own source offset renders at.
    // It cannot be compared against the pre-tap glyph: by design the caret line
    // becomes source-visible, revealing folded markers and dropping the heading
    // scale, so the whole line re-lays-out at the moment of the tap.
    const caretRect = editor.shadowRoot.querySelector(".visual-caret").getBoundingClientRect();
    const settledRect = glyphRect(
      projection.children[lineIndex],
      utf16IndexForByte(editor.value.split("\n")[lineIndex], head.byteInLine),
    );
    check(
      Boolean(settledRect) && Math.abs(caretRect.top - settledRect.top) <= 2,
      `[${theme}] drawn caret shares its source offset's rendered row on line ${lineIndex}`,
    );
    count += 1;
    check(
      Boolean(settledRect) && Math.abs(caretRect.left - settledRect.left) <= 2,
      `[${theme}] drawn caret sits at its source offset's rendered glyph on line ${lineIndex}`,
    );
    count += 1;
  }

  dispose();
  return count;
}

/**
 * A collapsed caret move performed by the platform raises `selectionchange` but
 * never `select`. The engine must still adopt it, or the drawn caret goes stale
 * while typing lands somewhere else.
 */
async function runCaretSyncCase(ContinuityEditorElement, check, mount) {
  const { editor, input, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "# Heading line\nsecond line of prose\nthird line of prose\n",
  );
  let count = 0;

  editor.setSelections(
    [{ anchor: { line: 0, byteInLine: 0 }, head: { line: 0, byteInLine: 0 }, kind: "caret" }],
    { reveal: false },
  );
  await settle();

  const target = editor.value.indexOf("third");
  // Move the caret the way the platform does: mutate the textarea selection and
  // announce it only through `selectionchange`.
  const release = suppressSelectEvent(editor.shadowRoot);
  input.setSelectionRange(target, target);
  document.dispatchEvent(new Event("selectionchange"));
  await settle();
  release();

  const head = editor.snapshot().selections[0].head;
  check(
    head.line === 2 && head.byteInLine === 0,
    `a collapsed platform caret move reaches the engine via selectionchange (got ${head.line}:${head.byteInLine})`,
  );
  count += 1;

  // Typing must land where the caret is drawn.
  const caretOffsetBefore = input.selectionStart;
  check(
    caretOffsetBefore === target,
    "textarea and engine agree after a collapsed platform caret move",
  );
  count += 1;

  dispose();
  return count;
}

/** Long-press must select through the projection, not the textarea's layout. */
async function runLongPressCases(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "# Title\nprose with **bold text** and more words following it\ntail line\n",
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  const element = projection.children[1];
  const rendered = element.textContent;
  const wordStart = rendered.indexOf("bold");
  const rect = glyphRect(element, wordStart + 1);
  const clientX = rect.left + rect.width / 2;
  const clientY = rect.top + rect.height / 2;

  // Claim must come from the gesture's own timer. Android Chrome raises no
  // `contextmenu` for a long-press on a textarea, so a contextmenu-only claim
  // is dead code on a phone.
  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  await longPress();

  const selectStart = new Event("selectstart", { bubbles: true, cancelable: true });
  input.dispatchEvent(selectStart);
  check(
    selectStart.defaultPrevented,
    "a matured long-press suppresses the platform's own selectstart",
  );
  count += 1;

  const selected = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    selected === "bold",
    `long-press selects the projected word under the finger (got ${JSON.stringify(selected)})`,
  );
  count += 1;

  // Drag-extend must follow projected glyphs. The line is source-visible now
  // that it carries the caret, so re-read its rendering rather than reusing the
  // folded indices captured before the press.
  const revealed = element.textContent;
  const endRect = glyphRect(element, revealed.indexOf("more") + 4);
  const move = pointerEvent(
    "pointermove", endRect.left + endRect.width - 1, endRect.top + endRect.height / 2,
  );
  input.dispatchEvent(move);
  await settle();
  const extended = input.value.slice(input.selectionStart, input.selectionEnd);
  check(
    move.defaultPrevented && extended.includes("more"),
    `long-press drag extends through projected glyphs (got ${JSON.stringify(extended)})`,
  );
  count += 1;

  const highlights = editor.shadowRoot.querySelectorAll(".visual-selection");
  check(highlights.length > 0, "an extended long-press selection paints a highlight");
  count += 1;
  const highlightRect = highlights[0]?.getBoundingClientRect();
  check(
    Boolean(highlightRect) && Math.abs(highlightRect.top - rect.top) <= rect.height,
    "the painted highlight starts on the row the finger touched",
  );
  count += 1;

  input.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));

  // A mouse right-click must keep the native context menu.
  const mouseMenu = new MouseEvent("contextmenu", {
    bubbles: true, cancelable: true, clientX, clientY,
  });
  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY, { pointerType: "mouse" }));
  input.dispatchEvent(mouseMenu);
  check(
    !mouseMenu.defaultPrevented,
    "a mouse right-click still yields the platform context menu",
  );
  count += 1;

  dispose();
  return count;
}
async function runCompositionTapCase(ContinuityEditorElement, check, mount) {
  const { editor, input, projection, dispose } = await mountEditor(
    ContinuityEditorElement, mount,
    "# Heading\nfirst prose line with **bold** inside\nsecond prose line\n",
  );
  let count = 0;
  placeCaret(input, editor.value.length);
  await settle();

  const element = projection.children[1];
  const rendered = element.textContent;
  const charIndex = rendered.indexOf("bold");
  const rect = glyphRect(element, charIndex);

  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  tap(input, rect.left + rect.width * 0.2, rect.top + rect.height / 2);
  await settle();
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "" }));
  await settle();

  const head = editor.snapshot().selections[0].head;
  const sourceLine = editor.value.split("\n")[1];
  const expectedByte = projectedCharToSourceByte(sourceLine, rendered, charIndex);
  check(
    head.line === 1 && Math.abs(head.byteInLine - expectedByte) <= 1,
    `a tap deferred behind composition replays onto the projected glyph (got ${head.line}:${head.byteInLine}, expected 1:${expectedByte})`,
  );
  count += 1;

  dispose();
  return count;
}
