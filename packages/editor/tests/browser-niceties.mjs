/** Exercise editing and presentation behaviors that require a real Chromium layout engine. */
export async function runBrowserNiceties(ContinuityEditorElement, check) {
  let assertions = 0;
  const editor = new ContinuityEditorElement();
  editor.style.cssText = "display:block;width:360px;height:180px";
  document.body.append(editor);
  await editor.ready;
  const input = editor.shadowRoot.querySelector("textarea");
  check(input.spellcheck === true, "spellcheck follows the browser default"); assertions += 1;
  editor.spellcheck = false;
  check(input.spellcheck === false && editor.getAttribute("spellcheck") === "false",
    "spellcheck property disables the semantic input"); assertions += 1;
  editor.setAttribute("spellcheck", "true");
  check(input.spellcheck === true, "spellcheck attribute re-enables the semantic input"); assertions += 1;
  editor.spellcheck = false;

  await smartEnter(editor, input, "    alpha");
  check(editor.value === "    alpha\n    ", "Enter preserves leading indentation"); assertions += 1;
  await smartEnter(editor, input, "- item");
  check(editor.value === "- item\n- ", "Enter continues a bullet"); assertions += 1;
  await smartEnter(editor, input, "- [x] done");
  check(editor.value === "- [x] done\n- [ ] ", "Enter continues a checked task unchecked"); assertions += 1;
  await smartEnter(editor, input, "- [ ] ");
  check(editor.value === "", "Enter on an empty task ends the list"); assertions += 1;

  editor.value = "alpha\nbeta\ngamma";
  await animationFrames(2);
  input.setSelectionRange(2, 2);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  const addDown = new KeyboardEvent("keydown", {
    key: "ArrowDown", bubbles: true, cancelable: true, ctrlKey: true, altKey: true,
  });
  check(!input.dispatchEvent(addDown), "Ctrl+Alt+Down is editor-owned"); assertions += 1;
  check(editor.snapshot().selections.length === 2, "Ctrl+Alt+Down adds a caret"); assertions += 1;
  check(editor.snapshot().selections[0].head.line === 1 && input.selectionStart === 8,
    "Ctrl+Alt+Down keeps the added caret active"); assertions += 1;
  await dispatchBeforeInput(editor, input, "insertText", "!", "multi-cursor typing");
  check(editor.value === "al!pha\nbe!ta\ngamma", "typing applies at every caret"); assertions += 1;
  check(editor.shadowRoot.querySelectorAll(".secondary-caret").length === 1, "secondary caret is visible"); assertions += 1;
  const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
  check(!input.dispatchEvent(escape), "first Escape clears secondary carets"); assertions += 1;
  check(editor.snapshot().selections.length === 1, "Escape leaves one semantic caret"); assertions += 1;

  const wrappedLine = "wrap ".repeat(40);
  editor.value = `${wrappedLine}\n${wrappedLine}`;
  await animationFrames(2);
  input.setSelectionRange(2, 2);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "ArrowDown", bubbles: true, cancelable: true, ctrlKey: true, altKey: true,
  }));
  const wrappedSelections = editor.snapshot().selections;
  check(wrappedSelections[0].head.line === 0 && wrappedSelections[0].head.byteInLine > 2,
    "Ctrl+Alt+Down advances one wrapped visual row"); assertions += 1;
  input.dispatchEvent(escape);
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "ArrowUp", bubbles: true, cancelable: true, ctrlKey: true, altKey: true,
  }));
  input.scrollTop = input.scrollHeight;
  input.dispatchEvent(new Event("scroll", { bubbles: true }));
  await animationFrames(2);
  const inputBounds = input.getBoundingClientRect();
  const wrappedCaretBounds = editor.shadowRoot.querySelector(".secondary-caret").getBoundingClientRect();
  check(wrappedCaretBounds.left >= inputBounds.left && wrappedCaretBounds.right <= inputBounds.right
    && wrappedCaretBounds.top >= inputBounds.top && wrappedCaretBounds.bottom <= inputBounds.bottom,
  `wrapped secondary caret follows the scrolled textarea viewport: ${JSON.stringify({
    input: boundsReport(inputBounds), caret: boundsReport(wrappedCaretBounds),
    scrollTop: input.scrollTop, selections: editor.snapshot().selections,
  })}`); assertions += 1;
  input.dispatchEvent(escape);

  const selectionsBeforeClick = editor.snapshot().selections.length;
  input.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, ctrlKey: true }));
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
  check(editor.snapshot().selections.length === selectionsBeforeClick + 1, "Ctrl+click adds a caret outside links"); assertions += 1;

  editor.value = "- [ ] clickable task\nplain";
  await animationFrames(2);
  input.setSelectionRange(2, 2);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  check(editor.value.startsWith("- [x] clickable task"), "clicking a task marker toggles it"); assertions += 1;

  editor.value = [
    "# Largest heading",
    "## Smaller heading",
    "**rendered bold** remains",
    "    indented text that wraps across the narrow editor width several times",
    "- list text that also wraps across the narrow editor width several times",
    "inline `copy me` sample",
    "```txt",
    "block copy text",
    "```",
    "plain",
  ].join("\n");
  await animationFrames(3);
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await delay(300);
  const lines = editor.shadowRoot.querySelectorAll(".projection .line");
  const firstHeadingSize = parseFloat(getComputedStyle(lines[0]).fontSize);
  const secondHeadingSize = parseFloat(getComputedStyle(lines[1]).fontSize);
  check(firstHeadingSize > secondHeadingSize, "heading levels have visible size hierarchy"); assertions += 1;
  check(lines[3].style.getPropertyValue("--continuity-wrap-indent").endsWith("px"),
    "indented wraps use measured browser pixels"); assertions += 1;
  check(lines[4].style.getPropertyValue("--continuity-wrap-indent").endsWith("px"),
    "list wraps use measured marker pixels"); assertions += 1;
  const listContentRect = textRange(lines[4], 2, 3).getBoundingClientRect();
  const listRowRects = [...textRange(lines[4], 0, lines[4].textContent.length).getClientRects()];
  check(listRowRects.length >= 2
    && Math.abs(listContentRect.left - listRowRects[1].left) <= 0.75,
  "list continuation rows align with first-row content"); assertions += 1;

  const copyButtons = editor.shadowRoot.querySelectorAll(".code-copy");
  check(copyButtons.length === 2, "inline and fenced code expose copy affordances"); assertions += 1;
  check(
    [...copyButtons].every((button) => button.getAttribute("aria-label")?.startsWith("Copy")),
    "copy affordances have accessible names",
  ); assertions += 1;

  for (const character of "abc") {
    await dispatchBeforeInput(editor, input, "insertText", character, "continuous typing");
    check(lines[0].textContent === "Largest heading"
      && lines[0].classList.contains("block-heading-1")
      && lines[2].querySelector(".inline-strong")?.textContent === "rendered bold",
    "continuous typing preserves Markdown projection on untouched lines"); assertions += 1;
  }
  check(lines[0].dataset.sourceVisible === "false"
    && lines[2].dataset.sourceVisible === "false"
    && lines[lines.length - 1].dataset.sourceVisible === "true",
  "fast edits reveal only the active or dirty line"); assertions += 1;

  editor.value = `- ${"a".repeat(180)}\nanchor`;
  await animationFrames(3);
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await delay(300);
  const unbrokenList = editor.shadowRoot.querySelector('.projection .line[data-line="0"]');
  const markerRect = textRange(unbrokenList, 0, 1).getBoundingClientRect();
  const firstTokenRect = textRange(unbrokenList, 2, 3).getBoundingClientRect();
  check(Math.abs(markerRect.top - firstTokenRect.top) <= 0.75,
    "unbroken list token starts beside its marker"); assertions += 1;

  editor.value = "";
  await animationFrames(2);
  input.setSelectionRange(0, 0);
  const bulkText = Array.from(
    { length: 600 },
    (_, index) => `line ${index}: wrapped text `.repeat(index % 4 + 1),
  ).join("\n");
  await dispatchBeforeInput(editor, input, "insertText", bulkText, "large paste");
  await animationFrames(2);
  const projection = editor.shadowRoot.querySelector(".projection");
  const inputRect = input.getBoundingClientRect();
  const visibleLines = [...projection.children].filter((line) => {
    const rect = line.getBoundingClientRect();
    return rect.bottom > inputRect.top && rect.top < inputRect.bottom;
  });
  const inputRange = input.scrollHeight - input.clientHeight;
  const residual = Number.parseFloat(input.dataset.scrollExtentResidual ?? "0");
  const scrollRatio = inputRange > 0
    ? Math.min(1, Math.max(0, input.scrollTop / inputRange))
    : 0;
  const projectionOffset = input.scrollTop
    + residual * scrollRatio;
  check(input.scrollTop > 0
      && projection.scrollHeight + 1 >= projectionOffset + input.clientHeight,
    `large paste keeps projection geometry over the revealed caret viewport: ${JSON.stringify({
      inputScrollTop: input.scrollTop,
      inputClientHeight: input.clientHeight,
      inputScrollHeight: input.scrollHeight,
      projectionScrollHeight: projection.scrollHeight,
      projectionOffset,
      residual,
      selectionStart: input.selectionStart,
      selectionEnd: input.selectionEnd,
    })}`); assertions += 1;
  check(visibleLines.length > 0 && visibleLines.every((line) => line.textContent.length > 0),
    "large paste paints text for every intersecting projection line"); assertions += 1;
  check(visibleLines.every((line) => !line.checkVisibility || line.checkVisibility({
    contentVisibilityAuto: true, opacityProperty: true, visibilityProperty: true,
  })), "large-paste viewport lines are paintable"); assertions += 1;

  const finalLine = bulkText.slice(bulkText.lastIndexOf("\n") + 1);
  editor.setSelections([{
    anchor: { line: 0, byteInLine: 0 },
    head: { line: 599, byteInLine: finalLine.length },
    kind: "caret",
  }], { reveal: false });
  await animationFrames(2);
  input.scrollTop = Math.floor((input.scrollHeight - input.clientHeight) / 2);
  input.dispatchEvent(new Event("scroll", { bubbles: true }));
  await animationFrames(2);
  const visualSelectionCount = editor.shadowRoot.querySelectorAll(".visual-selection").length;
  const middleVisibleLines = [...projection.children].filter((line) => {
    const rect = line.getBoundingClientRect();
    return rect.bottom > inputRect.top && rect.top < inputRect.bottom;
  });
  check(visualSelectionCount > 0 && visualSelectionCount < 200,
    "large selection paints only viewport-scoped visual rectangles"); assertions += 1;
  check(middleVisibleLines.some((line) => line.dataset.sourceVisible === "false"),
    "large selection reveals source only at its endpoints"); assertions += 1;

  editor.destroy();
  editor.remove();
  return assertions;
}

async function smartEnter(editor, input, source) {
  editor.value = source;
  await animationFrames(2);
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  const frame = new Promise((resolve) => editor.addEventListener("continuity-frame", resolve, { once: true }));
  const enter = new KeyboardEvent("keydown", {
    key: "Enter", code: "Enter", bubbles: true, cancelable: true,
  });
  if (input.dispatchEvent(enter)) {
    throw new Error(`physical Enter was not claimed for ${JSON.stringify(source)}`);
  }
  input.dispatchEvent(new KeyboardEvent("keyup", { key: "Enter", code: "Enter", bubbles: true }));
  await Promise.race([
    frame,
    delay(2_000).then(() => { throw new Error(`timed out waiting for smart Enter for ${JSON.stringify(source)}`); }),
  ]);
}

async function dispatchBeforeInput(editor, input, inputType, data = null, label = inputType) {
  const frame = new Promise((resolve) => editor.addEventListener("continuity-frame", resolve, { once: true }));
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true, cancelable: true, data, inputType,
  }));
  await Promise.race([
    frame,
    delay(2_000).then(() => { throw new Error(`timed out waiting for ${label}`); }),
  ]);
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => count-- <= 0 ? resolve() : requestAnimationFrame(next);
    next();
  });
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function textRange(element, start, end) {
  const range = document.createRange();
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let offset = 0;
  let startNode = element;
  let startOffset = 0;
  let endNode = element;
  let endOffset = element.childNodes.length;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const nextOffset = offset + node.data.length;
    if (start >= offset && start <= nextOffset) {
      startNode = node;
      startOffset = start - offset;
    }
    if (end >= offset && end <= nextOffset) {
      endNode = node;
      endOffset = end - offset;
      break;
    }
    offset = nextOffset;
  }
  range.setStart(startNode, startOffset);
  range.setEnd(endNode, endOffset);
  return range;
}

function boundsReport(bounds) {
  return { left: bounds.left, right: bounds.right, top: bounds.top, bottom: bounds.bottom };
}
