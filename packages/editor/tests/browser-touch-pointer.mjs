// Touch pointer contract. A finger is not a mouse: touch pointerdown must keep
// its native defaults without focusing, a resolved tap must synchronously map
// the projected caret and focus, a moving/canceled finger must never focus or
// mouse-drag-select, and mouse/pen capture behavior must remain unchanged.
export async function runTouchPointerTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const mount = document.querySelector("#mount");
  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", "Touch pointer document");
  editor.value = "# Title line\nplain middle line\ntail line\n";
  mount.append(editor);
  await editor.ready;
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  editor.scrollIntoView({ block: "center" });

  // Park the caret on the last line so the heading stays projected
  // (markers hidden, scaled) — the divergent case a raw-layout tap gets wrong.
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await animationFrames(3);
  const heading = shadow.querySelector('.projection [data-line="0"]');
  const glyphAt = heading.textContent.indexOf("Title");
  const walker = document.createTreeWalker(heading, NodeFilter.SHOW_TEXT);
  let consumed = 0;
  let glyphRect = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (glyphAt >= consumed && glyphAt < consumed + node.length) {
      const range = document.createRange();
      range.setStart(node, glyphAt - consumed);
      range.setEnd(node, glyphAt - consumed + 1);
      glyphRect = range.getBoundingClientRect();
      break;
    }
    consumed += node.length;
  }
  const tapX = glyphRect.left + glyphRect.width * 0.2;
  const tapY = glyphRect.top + glyphRect.height / 2;
  input.blur();
  let focusEvents = 0;
  input.addEventListener("focus", () => { focusEvents += 1; });

  const touch = (type, clientX, clientY, extra = {}) => {
    const event = new PointerEvent(type, {
      bubbles: true, cancelable: true, pointerType: "touch", pointerId: 7,
      isPrimary: true, button: 0, buttons: 1, clientX, clientY, ...extra,
    });
    input.dispatchEvent(event);
    return event;
  };

  const down = touch("pointerdown", tapX, tapY);
  check(down.defaultPrevented === false,
    "touch pointerdown keeps native defaults"); assertions += 1;
  check(editor.shadowRoot.activeElement !== input && focusEvents === 0,
    "touch pointerdown does not focus or briefly open the keyboard"); assertions += 1;
  touch("click", tapX + 10, tapY + 4, { buttons: 0, detail: 1 });
  const expectedCaret = editor.value.indexOf("Title");
  check(input.selectionStart === expectedCaret && input.selectionEnd === expectedCaret,
    `a jittered touch tap maps the caret through the projected glyph (caret=${input.selectionStart}, expected=${expectedCaret})`);
  assertions += 1;
  check(editor.shadowRoot.activeElement === input && focusEvents === 1,
    "a resolved touch tap synchronously focuses the semantic input"); assertions += 1;

  const caretBeforeMove = input.selectionStart;
  input.blur();
  focusEvents = 0;
  touch("pointerdown", tapX, tapY);
  const move = touch("pointermove", tapX + 30, tapY + 60);
  check(move.defaultPrevented === false
    && input.selectionStart === caretBeforeMove && input.selectionEnd === caretBeforeMove,
  "a moving finger never mouse-drag-selects or cancels the pan"); assertions += 1;
  touch("pointerup", tapX + 30, tapY + 60, { buttons: 0 });
  touch("click", tapX + 30, tapY + 60, { buttons: 0, detail: 1 });
  check(editor.shadowRoot.activeElement !== input && focusEvents === 0,
    "a touch pan remains unfocused even if a trailing click is delivered"); assertions += 1;

  // A canceled touch (scroll took the pointer) must drop tap state, so the
  // next native selection event cannot collapse existing multi-carets.
  editor.setSelections([
    { anchor: { line: 0, byteInLine: 2 }, head: { line: 0, byteInLine: 2 }, kind: "caret" },
    { anchor: { line: 1, byteInLine: 0 }, head: { line: 1, byteInLine: 0 }, kind: "caret" },
  ], { reveal: false });
  touch("pointerdown", tapX, tapY + 40);
  touch("pointercancel", tapX + 12, tapY + 90, { buttons: 0 });
  touch("click", tapX + 12, tapY + 90, { buttons: 0, detail: 1 });
  input.dispatchEvent(new Event("select", { bubbles: true }));
  check(editor.snapshot().selections.length === 2,
    "pointercancel drops tap state so native selection keeps multi-carets"); assertions += 1;
  check(editor.shadowRoot.activeElement !== input && focusEvents === 0,
    "a canceled touch never focuses the semantic input"); assertions += 1;

  const mouseDown = new PointerEvent("pointerdown", {
    bubbles: true, cancelable: true, pointerType: "mouse", pointerId: 8,
    isPrimary: true, button: 0, buttons: 1, clientX: tapX, clientY: tapY,
  });
  input.dispatchEvent(mouseDown);
  check(mouseDown.defaultPrevented === true,
    "mouse pointerdown still uses projection-owned capture"); assertions += 1;
  input.dispatchEvent(new PointerEvent("click", {
    bubbles: true, pointerType: "mouse", pointerId: 8, button: 0,
    clientX: tapX, clientY: tapY, detail: 1,
  }));

  input.blur();
  const penDown = new PointerEvent("pointerdown", {
    bubbles: true, cancelable: true, pointerType: "pen", pointerId: 9,
    isPrimary: true, button: 0, buttons: 1, clientX: tapX, clientY: tapY,
  });
  input.dispatchEvent(penDown);
  check(penDown.defaultPrevented === true && editor.shadowRoot.activeElement === input,
    "pen pointerdown still focuses and uses projection-owned capture"); assertions += 1;

  editor.destroy();
  editor.remove();
  return assertions;
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => count-- <= 0 ? resolve() : requestAnimationFrame(next);
    requestAnimationFrame(next);
  });
}
