// Touch shield contract.
//
// A finger resting on the textarea gets the platform's own long-press
// selection, hit-tested against a layout that cannot match the projection —
// folded markers, headings at up to 1.45em, a narrower box. Nothing refuses it
// on an editable element: `selectstart` is not raised for it, `contextmenu`
// arrives after the selection already happened, and `user-select` does not
// apply. So on a coarse pointer the finger lands on a plain non-editable shield
// instead, which honours `user-select: none` and owns the scrolling.
//
// These assertions only mean anything with a coarse primary pointer, so the
// runner re-runs this file under touch emulation.
export async function runTouchShieldTests(ContinuityEditorElement, check) {
  if (!matchMedia("(pointer: coarse)").matches) return 0;
  let assertions = 0;
  const mount = document.querySelector("#mount");
  const long = "The quick brown fox jumps over the lazy dog while the writer keeps typing";

  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", "Touch shield document");
  editor.setAttribute("command-rail", "on");
  editor.value = Array.from({ length: 12 }, (_, index) => [
    `## Section ${index}: ${long}`,
    `${long} body ${index} with **bold text**, \`inline code\`, and [a link](https://example.com/t/${index}).`,
    `- list item ${index} that wraps beneath the bullet hanging indent for a while`,
    "",
  ].join("\n")).join("\n");
  mount.append(editor);
  await editor.ready;
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  const projection = shadow.querySelector(".projection");
  const frame = shadow.querySelector(".frame");
  const shield = shadow.querySelector(".touch-shield");
  const spacer = shadow.querySelector(".touch-shield-spacer");
  input.focus();
  await settle();

  check(frame.classList.contains("touch-scrolling"),
    "a coarse pointer routes the editor through the touch shield"); assertions += 1;
  check(getComputedStyle(shield).pointerEvents === "auto"
    && getComputedStyle(input).pointerEvents === "none",
  "the shield takes the finger and the textarea stops taking it"); assertions += 1;
  check(getComputedStyle(shield).webkitUserSelect === "none",
    "the shield refuses the platform's own selection"); assertions += 1;

  // The scrollable extent is the projection's height by construction, which is
  // what removes the class of bug where the document's tail was unreachable.
  check(Math.abs(parseFloat(spacer.style.height) - projection.offsetHeight) <= 1,
    `the shield spacer tracks the projection (${spacer.style.height} vs ${projection.offsetHeight}px)`);
  assertions += 1;

  // --- Long-press selects the projected word, and the drag extends it. ---
  const element = projection.children[1];
  const rendered = element.textContent;
  const rect = glyphRect(element, rendered.indexOf("brown") + 2);
  const clientX = rect.left + rect.width / 2;
  const clientY = rect.top + rect.height / 2;
  const touch = (type, x, y, extra = {}) => {
    const event = new PointerEvent(type, {
      bubbles: true, cancelable: true, pointerType: "touch", pointerId: 31,
      isPrimary: true, button: 0, buttons: 1, clientX: x, clientY: y, ...extra,
    });
    shield.dispatchEvent(event);
    return event;
  };

  // --- A tap focuses only after resolution; a pan/cancel never focuses. ---
  const tapRect = glyphRect(element, rendered.indexOf("brown") + 2);
  const tapX = tapRect.left + tapRect.width * 0.2;
  const tapY = tapRect.top + tapRect.height / 2;
  input.blur();
  let focusEvents = 0;
  input.addEventListener("focus", () => { focusEvents += 1; });

  touch("pointerdown", tapX, tapY);
  check(shadow.activeElement !== input && focusEvents === 0,
    "shield pointerdown leaves the editor unfocused for native scrolling"); assertions += 1;
  touch("pointerup", tapX, tapY, { buttons: 0 });
  check(shadow.activeElement !== input && focusEvents === 0,
    "touch pointerup alone does not open the keyboard before tap resolution"); assertions += 1;
  touch("click", tapX, tapY, { buttons: 0, detail: 1 });
  const expectedTapCaret = input.value.indexOf("brown", input.value.indexOf("\n") + 1) + 2;
  check(input.selectionStart === expectedTapCaret && input.selectionEnd === expectedTapCaret,
    `a shield tap places the projected caret (${input.selectionStart} vs ${expectedTapCaret})`);
  assertions += 1;
  check(shadow.activeElement === input && focusEvents === 1,
    "a resolved shield tap synchronously focuses the semantic input"); assertions += 1;

  input.blur();
  focusEvents = 0;
  touch("pointerdown", tapX, tapY);
  touch("pointermove", tapX + 30, tapY + 60);
  touch("pointerup", tapX + 30, tapY + 60, { buttons: 0 });
  touch("click", tapX + 30, tapY + 60, { buttons: 0, detail: 1 });
  check(shadow.activeElement !== input && focusEvents === 0,
    "a shield pan stays unfocused even if an engine emits a trailing click"); assertions += 1;

  touch("pointerdown", tapX, tapY);
  touch("pointercancel", tapX, tapY + 60, { buttons: 0 });
  touch("click", tapX, tapY + 60, { buttons: 0, detail: 1 });
  check(shadow.activeElement !== input && focusEvents === 0,
    "a canceled shield gesture never focuses the semantic input"); assertions += 1;
  input.focus();
  await settle();

  touch("pointerdown", clientX, clientY);
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
  check(input.value.slice(input.selectionStart, input.selectionEnd) === "brown",
    `a long-press on the shield selects the projected word (got ${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd))})`);
  assertions += 1;

  // The shield is a real scroller. A synthetic `pointermove` never triggers
  // panning, so only the `touchmove` default tells us whether a real finger
  // would have scrolled the surface out from under the drag.
  const scrollAttempt = new TouchEvent("touchmove", { bubbles: true, cancelable: true });
  shield.dispatchEvent(scrollAttempt);
  check(scrollAttempt.defaultPrevented,
    "a claimed selection refuses the scroll so the drag survives the finger moving");
  assertions += 1;

  const forward = glyphRect(element, rendered.indexOf("jumps") + 5);
  const move = touch(
    "pointermove", forward.left + forward.width - 1, forward.top + forward.height / 2,
  );
  await settle();
  const dragged = input.value.slice(input.selectionStart, input.selectionEnd);
  check(move.defaultPrevented && dragged.startsWith("brown") && dragged.includes("jumps"),
    `dragging on the shield extends through projected glyphs (got ${JSON.stringify(dragged)})`);
  assertions += 1;
  touch("pointerup", clientX, clientY, { buttons: 0 });

  // --- The document tail must be reachable, clear of the command rail. ---
  let previous = -1;
  for (let attempt = 0; attempt < 15 && shield.scrollTop !== previous; attempt += 1) {
    previous = shield.scrollTop;
    shield.scrollTop = shield.scrollHeight;
    shield.dispatchEvent(new Event("scroll"));
    await animationFrames(4);
  }
  await settle();
  shield.scrollTop = shield.scrollHeight;
  shield.dispatchEvent(new Event("scroll"));
  await animationFrames(4);

  const rail = shadow.querySelector(".command-rail");
  const floor = rail && !rail.hidden
    ? rail.getBoundingClientRect().top
    : frame.getBoundingClientRect().bottom;
  const children = [...projection.children];
  const lastBounds = children[children.length - 1].getBoundingClientRect();
  check(lastBounds.bottom <= floor + 1,
    `the last line clears the rail at the scroll floor (overshoot ${Math.round(lastBounds.bottom - floor)}px)`);
  assertions += 1;
  const hidden = children.filter(
    (child) => child.getBoundingClientRect().top >= floor - 1,
  ).length;
  check(hidden === 0, `no trailing line is stranded behind the rail (${hidden})`); assertions += 1;

  // The projection must follow the shield, not the textarea.
  check(getComputedStyle(projection).transform.includes(`-${Math.round(shield.scrollTop)}`)
    || Math.abs(readTranslateY(projection) + shield.scrollTop) <= 2,
  "the projection is translated by the shield's scroll offset"); assertions += 1;

  // --- Edge auto-scroll. Refusing the scroll means the finger cannot reach
  // past the viewport on its own, so the surface has to move for it. ---
  shield.scrollTop = 0;
  shield.dispatchEvent(new Event("scroll"));
  await settle();
  const firstLine = projection.children[0];
  const anchorRect = glyphRect(firstLine, 4);
  touch("pointerdown", anchorRect.left + 2, anchorRect.top + anchorRect.height / 2);
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
  const scrollBeforeEdge = shield.scrollTop;
  const shieldBounds = shield.getBoundingClientRect();
  // Hold the finger inside the bottom edge band.
  touch("pointermove", shieldBounds.left + 40, shieldBounds.bottom - 8);
  await new Promise((resolve) => setTimeout(resolve, 260));
  check(shield.scrollTop > scrollBeforeEdge,
    `a drag held at the bottom edge scrolls the surface (${scrollBeforeEdge} -> ${shield.scrollTop})`);
  assertions += 1;
  const grown = input.value.slice(input.selectionStart, input.selectionEnd);
  check(grown.length > 0 && input.selectionEnd > input.selectionStart,
    "the selection keeps extending while the surface auto-scrolls"); assertions += 1;
  touch("pointerup", shieldBounds.left + 40, shieldBounds.bottom - 8, { buttons: 0 });
  await settle();
  const settledScroll = shield.scrollTop;
  await new Promise((resolve) => setTimeout(resolve, 200));
  check(shield.scrollTop === settledScroll,
    "auto-scroll stops once the finger lifts"); assertions += 1;

  // --- Selection actions replace the platform bubble the shield displaces. ---
  const bar = shadow.querySelector(".selection-actions");
  check(Boolean(bar) && !bar.hidden,
    "a settled selection offers the clipboard action bar"); assertions += 1;
  const labels = [...bar.querySelectorAll(".selection-actions-button")]
    .filter((button) => !button.hidden)
    .map((button) => button.textContent);
  check(labels.includes("Copy") && labels.includes("Cut") && labels.includes("Select all"),
    `the bar offers the clipboard actions (${labels.join(", ")})`); assertions += 1;
  const barBounds = bar.getBoundingClientRect();
  const frameBounds = frame.getBoundingClientRect();
  check(barBounds.left >= frameBounds.left - 1 && barBounds.right <= frameBounds.right + 1,
    "the action bar stays inside the frame"); assertions += 1;

  // A real finger, not `.click()`. A programmatic click skips `pointerdown`
  // and fires on elements that hit-testing would never reach, so it cannot see
  // a control that dismisses itself mid-tap.
  // The precise invariant: a press on the bar must not dismiss it. Pointer
  // listeners live on the frame, the bar lives inside the frame, and hiding it
  // on its own press sets `display: none` — so the button is gone before the
  // click can reach it and every action silently does nothing.
  const selectAllButton = bar.querySelector('[data-selection-action="select-all"]');
  selectAllButton.dispatchEvent(new PointerEvent("pointerdown", {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 45,
    isPrimary: true, button: 0, buttons: 1,
  }));
  check(!bar.hidden, "pressing the bar does not dismiss it before the tap completes");
  assertions += 1;

  tapElement(selectAllButton);
  await settle();
  check(input.selectionStart === 0 && input.selectionEnd === input.value.length,
    "a finger tap on Select all covers the document"); assertions += 1;
  check(!bar.hidden, "the bar survives its own tap"); assertions += 1;

  const copied = [];
  const realClipboard = navigator.clipboard;
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: async (value) => { copied.push(value); } },
  });
  const expected = input.value.slice(input.selectionStart, input.selectionEnd);
  tapElement(bar.querySelector('[data-selection-action="copy"]'));
  await settle();
  check(copied.length === 1 && copied[0] === expected,
    `a finger tap on Copy writes the selection (${copied.length} write(s), match=${copied[0] === expected})`);
  assertions += 1;
  check(bar.hidden, "the bar dismisses once an action completes"); assertions += 1;
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: realClipboard });

  // --- A long-press that selects nothing still offers the clipboard. On a
  // phone this bar is the only route to Select all or Paste. ---
  const pasted = [];
  const realClipboard2 = navigator.clipboard;
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { readText: async () => "PASTED-TEXT", writeText: async () => {} },
  });
  editor.setSelections(
    [{ anchor: { line: 2, byteInLine: 0 }, head: { line: 2, byteInLine: 0 }, kind: "caret" }],
    { reveal: false },
  );
  await settle();
  check(bar.hidden, "a bare caret alone does not summon the bar"); assertions += 1;

  const caretLine = projection.children[2];
  const caretRect = glyphRect(caretLine, 3);
  touch("pointerdown", caretRect.left + 1, caretRect.top + caretRect.height / 2);
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
  touch("pointerup", caretRect.left + 1, caretRect.top + caretRect.height / 2, { buttons: 0 });
  await settle();
  check(!bar.hidden, "a long-press offers the bar even with nothing selected"); assertions += 1;
  const offered = [...bar.querySelectorAll(".selection-actions-button")]
    .filter((button) => !button.hidden).map((button) => button.dataset.selectionAction);
  check(offered.includes("select-all") && offered.includes("paste"),
    `a caret-only bar offers select-all and paste (${offered.join(",")})`); assertions += 1;

  const before = editor.value.length;
  tapElement(bar.querySelector('[data-selection-action="paste"]'));
  await settle();
  await new Promise((resolve) => setTimeout(resolve, 120));
  pasted.push(editor.value.length - before);
  check(editor.value.includes("PASTED-TEXT"),
    `paste inserts the clipboard text (grew by ${pasted[0]})`); assertions += 1;

  // An embedding frame that withholds `clipboard-read` denies the read outright
  // and no page-side code can lift it, so paste must still work for text this
  // editor copied itself.
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      readText: async () => { throw new Error("NotAllowedError: blocked by permissions policy"); },
      writeText: async () => {},
    },
  });
  editor.setSelections(
    [{ anchor: { line: 2, byteInLine: 0 }, head: { line: 2, byteInLine: 4 }, kind: "caret" }],
    { reveal: false },
  );
  await settle();
  const remembered = input.value.slice(input.selectionStart, input.selectionEnd);
  tapElement(bar.querySelector('[data-selection-action="copy"]'));
  await settle();
  editor.setSelections(
    [{ anchor: { line: 3, byteInLine: 0 }, head: { line: 3, byteInLine: 0 }, kind: "caret" }],
    { reveal: false },
  );
  await settle();
  touch("pointerdown", caretRect.left + 1, caretRect.top + caretRect.height / 2);
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
  touch("pointerup", caretRect.left + 1, caretRect.top + caretRect.height / 2, { buttons: 0 });
  await settle();
  // The long-press selected a word, so the paste replaces it: assert the
  // document changed and now carries the remembered text at that point, rather
  // than assuming it grew.
  const valueBeforeBlocked = editor.value;
  const pasteAt = input.selectionStart;
  tapElement(bar.querySelector('[data-selection-action="paste"]'));
  await settle();
  await new Promise((resolve) => setTimeout(resolve, 120));
  check(editor.value !== valueBeforeBlocked
    && editor.value.slice(pasteAt, pasteAt + remembered.length) === remembered,
  `paste still works when the browser denies clipboard-read (wanted ${JSON.stringify(remembered)} at ${pasteAt})`);
  assertions += 1;
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: realClipboard2 });

  // --- A selection taller than the viewport keeps the bar reachable. Placed
  // from the selection start alone, the bar left the screen the moment that
  // start scrolled off, and the only route back to the clipboard was to scroll
  // up and find it again. ---
  const lastLine = editor.snapshot().text.split("\n").length - 1;
  editor.setSelections([{
    anchor: { line: 0, byteInLine: 0 },
    head: { line: lastLine, byteInLine: 0 },
    kind: "caret",
  }], { reveal: false });
  shield.scrollTop = 0;
  shield.dispatchEvent(new Event("scroll"));
  await settle();
  check(!bar.hidden, "a document-spanning selection offers the bar"); assertions += 1;
  shield.scrollTop = Math.min(
    shield.scrollHeight - shield.clientHeight,
    Math.round(shield.clientHeight * 1.5),
  );
  shield.dispatchEvent(new Event("scroll"));
  await settle();
  const scrolledBar = bar.getBoundingClientRect();
  const scrolledFrame = frame.getBoundingClientRect();
  check(scrolledBar.top >= scrolledFrame.top - 1 && scrolledBar.bottom <= scrolledFrame.bottom + 1,
    `the bar stays on screen once the selection start scrolls off (bar ${Math.round(scrolledBar.top)}..${Math.round(scrolledBar.bottom)}, frame ${Math.round(scrolledFrame.top)}..${Math.round(scrolledFrame.bottom)})`);
  assertions += 1;

  editor.destroy();
  editor.remove();
  return assertions;
}

/**
 * Dispatch the pointer sequence a finger produces on a control, including the
 * `pointerdown` that a programmatic `.click()` never sends.
 */
function tapElement(element) {
  const bounds = element.getBoundingClientRect();
  const clientX = bounds.left + bounds.width / 2;
  const clientY = bounds.top + bounds.height / 2;
  const root = element.getRootNode();
  const send = (type, Constructor, extra) => {
    // Re-resolve the target by hit-test between events, exactly as the browser
    // does. Dispatching blindly at a saved reference still fires listeners on an
    // element that has since become `display: none`, which hides the very bug
    // where something dismisses the control mid-tap.
    const target = root.elementFromPoint?.(clientX, clientY) ?? element;
    if (!element.isConnected || target === null) return false;
    if (!element.contains(target) && target !== element) return false;
    target.dispatchEvent(new Constructor(type, {
      bubbles: true, cancelable: true, clientX, clientY, ...extra,
    }));
    return true;
  };
  const touch = { pointerType: "touch", pointerId: 44, isPrimary: true, button: 0 };
  if (!send("pointerdown", PointerEvent, { ...touch, buttons: 1 })) return;
  if (!send("pointerup", PointerEvent, { ...touch, buttons: 0 })) return;
  send("click", MouseEvent, { detail: 1 });
}

function readTranslateY(element) {
  const matrix = getComputedStyle(element).transform;
  const parts = matrix.startsWith("matrix(") ? matrix.slice(7, -1).split(",") : [];
  return parts.length === 6 ? parseFloat(parts[5]) : 0;
}

function glyphRect(element, charIndex) {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let consumed = 0;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (charIndex >= consumed && charIndex < consumed + node.length) {
      const range = document.createRange();
      range.setStart(node, charIndex - consumed);
      range.setEnd(node, charIndex - consumed + 1);
      return [...range.getClientRects()].filter((bounds) => bounds.height > 0)[0] ?? null;
    }
    consumed += node.length;
  }
  return null;
}

/** The Markdown projection lands on a 250ms idle timer after the fast pass. */
async function settle() {
  await animationFrames(2);
  await new Promise((resolve) => setTimeout(resolve, 320));
  await animationFrames(3);
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => (count-- <= 0 ? resolve() : requestAnimationFrame(next));
    requestAnimationFrame(next);
  });
}
