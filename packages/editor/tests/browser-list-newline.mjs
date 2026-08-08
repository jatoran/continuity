// Enter on a list line, with and without a live IME composition.
//
// Android keyboards hold a composition open on the word under the caret almost
// continuously, so anything gated behind "not composing" is gated off for the
// whole session on a phone. Enter was: the composing branch returned before the
// line-break routing without preventing the default, so the textarea inserted a
// raw newline, the list-aware planner never ran, and the split lost its marker.
// The tail then reads as a lazy continuation of the same item, which is correct
// CommonMark for text the writer never asked for.
//
// Every case below therefore runs twice: once with the composition closed (the
// desktop keydown route) and once with one open (the Android beforeinput
// route). The expectations are identical because the composition is incidental.

const CARET = "|";

// `source` carries the caret as `|`; it is stripped before the document loads.
const CASES = [
  { name: "end of an item", source: "- foo|", expected: "- foo\n- " },
  { name: "mid-word", source: "- foo ba|r", expected: "- foo ba\n- r" },
  { name: "at an interior space", source: "- foo |bar", expected: "- foo \n- bar" },
  { name: "empty item", source: "- |", expected: "" },
  { name: "mid-word in an unchecked task", source: "- [ ] f|oo", expected: "- [ ] f\n- [ ] oo" },
  { name: "end of a checked task", source: "- [x] foo|", expected: "- [x] foo\n- [ ] " },
  { name: "empty task stub", source: "- [ ] |", expected: "" },
  { name: "mid-word in a nested item", source: "  - al|pha", expected: "  - al\n  - pha" },
  { name: "deeply nested item", source: "    - beta|", expected: "    - beta\n    - " },
  { name: "ordered item", source: "1. one\n2. two|", expected: "1. one\n2. two\n3. " },
];

export async function runListNewlineTests(ContinuityEditorElement, check) {
  let assertions = 0;
  const editor = new ContinuityEditorElement();
  editor.value = "seed";
  editor.setAttribute("aria-label", "List newline document");
  editor.style.cssText = "display:block;width:420px;height:220px";
  document.body.append(editor);
  await editor.ready;
  const input = editor.shadowRoot.querySelector("textarea");

  for (const testCase of CASES) {
    for (const isComposing of [false, true]) {
      const lane = isComposing ? "composing" : "idle";
      await loadCase(editor, input, testCase);
      const claimed = isComposing ? pressComposingEnter(input) : pressEnter(input);
      await settle();
      check(claimed, `${testCase.name} (${lane}): the editor claims Enter`); assertions += 1;
      check(editor.snapshot().text === testCase.expected,
        `${testCase.name} (${lane}): ${JSON.stringify(editor.snapshot().text)} === ${JSON.stringify(testCase.expected)}`);
      assertions += 1;
      if (isComposing) {
        check(editor.composing === false,
          `${testCase.name}: Enter closes the composition it committed`); assertions += 1;
      }
    }
  }

  // The true Android state: the textarea already holds a composed run the
  // engine has not absorbed. Enter has to fold that run in before it plans the
  // break, or the split lands against text the engine no longer matches.
  await loadCase(editor, input, { source: "- foo|", expected: "" });
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  input.value = "- fooZZ";
  input.setSelectionRange(7, 7);
  input.dispatchEvent(new InputEvent("input", {
    bubbles: true, inputType: "insertCompositionText", data: "ZZ",
  }));
  check(editor.snapshot().text === "- foo", "the composing run stays out of the engine"); assertions += 1;
  const claimedRun = !input.dispatchEvent(lineBreakEvent());
  await settle();
  check(claimedRun, "a composing Enter over a live run is claimed"); assertions += 1;
  check(editor.snapshot().text === "- fooZZ\n- ",
    `Enter commits the composed run before splitting (${JSON.stringify(editor.snapshot().text)})`);
  assertions += 1;

  // Shift+Enter stays a raw newline: the writer is asking for a line break
  // inside the item, not for the next one.
  await loadCase(editor, input, { source: "- foo|", expected: "" });
  input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Enter", bubbles: true, cancelable: true, shiftKey: true,
  }));
  await settle();
  check(editor.snapshot().text === "- foo\n",
    `Shift+Enter inserts a raw newline (${JSON.stringify(editor.snapshot().text)})`); assertions += 1;

  editor.destroy();
  editor.remove();
  return assertions;
}

async function loadCase(editor, input, testCase) {
  const caret = testCase.source.indexOf(CARET);
  editor.value = testCase.source.replace(CARET, "");
  await settle();
  input.setSelectionRange(caret, caret);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await settle();
}

/** The desktop route: keydown owns Enter before any `beforeinput` is raised. */
function pressEnter(input) {
  return !input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Enter", bubbles: true, cancelable: true,
  }));
}

/**
 * The Android route. A soft-keyboard Enter arrives inside a composition, where
 * `keydown` reports `Unidentified` / keyCode 229 and carries no usable key, so
 * `beforeinput` is the only event that names the intent.
 */
function pressComposingEnter(input) {
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Unidentified", keyCode: 229, bubbles: true, cancelable: true,
  }));
  return !input.dispatchEvent(lineBreakEvent());
}

function lineBreakEvent() {
  return new InputEvent("beforeinput", {
    bubbles: true, cancelable: true, inputType: "insertLineBreak", data: null,
  });
}

function settle() {
  return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
}
