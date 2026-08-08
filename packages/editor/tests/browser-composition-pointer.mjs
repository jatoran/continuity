import { positionToUtf16Offset } from "/node_modules/@continuity-editor/editor/src/coordinates.js";

// Pointer placement during an active IME composition (the Android Gboard
// "tap lands one character off until you type a space" report). A tap taken
// while the engine still holds pre-composition text must hit-test against the
// live textarea line and be applied only once compositionend reconciles, so the
// caret lands on the tapped character rather than a stale-mapped byte. This
// covers both event orderings (compositionend after the click, and between
// pointerdown and click) plus a UTF-8/UTF-16 boundary. No keyboard, browser, or
// user-agent branch exists to test — the fix is deliberately input-agnostic.
export async function runCompositionPointerTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const mount = document.querySelector("#mount");

  const glyphRectInLine = (line, indexInText) => {
    const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
    let consumed = 0;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (indexInText >= consumed && indexInText < consumed + node.length) {
        const range = document.createRange();
        range.setStart(node, indexInText - consumed);
        range.setEnd(node, indexInText - consumed + 1);
        return range.getBoundingClientRect();
      }
      consumed += node.length;
    }
    return null;
  };

  const touch = (input, type, clientX, clientY) => {
    input.dispatchEvent(new PointerEvent(type, {
      bubbles: true, cancelable: true, pointerType: "touch", pointerId: 11,
      isPrimary: true, button: 0, buttons: type === "pointerdown" ? 1 : 0,
      clientX, clientY, detail: type === "click" ? 1 : 0,
    }));
  };

  async function scenario({ label, seed, word, tapIndexInWord, endBetween }) {
    const editor = new ContinuityEditorElement();
    editor.setAttribute("aria-label", label);
    editor.value = seed;
    mount.append(editor);
    await editor.ready;
    const shadow = editor.shadowRoot;
    const input = shadow.querySelector("textarea");
    const projection = shadow.querySelector("[part=projection]");
    editor.scrollIntoView({ block: "center" });

    // Compose `word` at the end of line 0 (a source-visible preview line).
    const composeAt = seed.indexOf("\n") < 0 ? seed.length : seed.indexOf("\n");
    input.focus();
    input.setSelectionRange(composeAt, composeAt);
    input.dispatchEvent(new Event("select", { bubbles: true }));
    input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    input.value = `${seed.slice(0, composeAt)}${word}${seed.slice(composeAt)}`;
    input.setSelectionRange(composeAt + word.length, composeAt + word.length);
    input.dispatchEvent(new InputEvent("input", {
      bubbles: true, inputType: "insertCompositionText", data: word,
    }));
    await animationFrames(2);

    // Locate a glyph inside the live composed word on the previewed line.
    const tapCharIndex = composeAt + tapIndexInWord;
    const rect = glyphRectInLine(projection.children[0], tapCharIndex);
    check(Boolean(rect), `${label}: composed glyph is measurable in the live preview`); assertions += 1;
    const tapX = rect.left + rect.width * 0.2;
    const tapY = rect.top + rect.height / 2;

    // The reported ordering: pointerdown, then the click fires while the engine
    // still holds pre-composition text, and compositionend arrives afterward.
    touch(input, "pointerdown", tapX, tapY);
    if (endBetween) {
      input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: word }));
    }
    touch(input, "click", tapX, tapY);
    if (!endBetween) {
      input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: word }));
    }

    const committed = editor.snapshot().text;
    check(committed.split(word).length - 1 === 1,
      `${label}: the composed word commits exactly once`); assertions += 1;

    // Tapping the left fifth of the glyph lands the caret before that character.
    const expectedCaret = tapCharIndex;
    const snapshot = editor.snapshot();
    check(input.selectionStart === expectedCaret && input.selectionEnd === expectedCaret,
      `${label}: the textarea caret lands on the tapped character (caret=${input.selectionStart}, expected=${expectedCaret})`);
    assertions += 1;
    const engineOffset = positionToUtf16Offset(committed, snapshot.selections[0].head);
    check(snapshot.selections.length === 1 && engineOffset === expectedCaret,
      `${label}: the engine caret agrees with the textarea (engine=${engineOffset}, expected=${expectedCaret})`);
    assertions += 1;

    // A subsequent character must land at exactly the tapped position.
    input.setRangeText("Z", input.selectionStart, input.selectionEnd, "end");
    input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: "Z" }));
    check(editor.snapshot().text.slice(0, expectedCaret + 1).endsWith("Z"),
      `${label}: the next character lands at the tapped position`); assertions += 1;

    editor.destroy();
    editor.remove();
  }

  // Ordering A: compositionend after the click (the deferred-tap path).
  await scenario({
    label: "Composition tap (end after click)",
    seed: "abc\nsecond line\n",
    word: "hello",
    tapIndexInWord: 2,
    endBetween: false,
  });
  // Ordering B: compositionend between pointerdown and click (the cached path).
  await scenario({
    label: "Composition tap (end between down and click)",
    seed: "abc\nsecond line\n",
    word: "world",
    tapIndexInWord: 3,
    endBetween: true,
  });
  // Unicode: UTF-8 engine bytes versus UTF-16 browser offsets on the tapped line.
  await scenario({
    label: "Composition tap (unicode line)",
    seed: "café\nsecond line\n",
    word: "über",
    tapIndexInWord: 2,
    endBetween: false,
  });

  return assertions;
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => (count-- <= 0 ? resolve() : requestAnimationFrame(next));
    requestAnimationFrame(next);
  });
}
