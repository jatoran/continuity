// Line-scoped IME composition lane. Mobile keyboards run a composition per
// word, so the projection must stay visible for the whole document while only
// the composing line previews the live textarea text; an unmappable composed
// run (for example one containing a newline) falls back to the frame-wide
// native reveal so the writer is never typing blind.
export async function runCompositionTests(editor, input, projection, check) {
  let assertions = 0;

  // Compose away from line 0 so the heading is a bystander line.
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await animationFrames(2);
  const headingStyleBeforeComposition = getComputedStyle(projection.children[0]).fontSize
    + projection.children[0].className;
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  check(editor.composing === true
    && !editor.shadowRoot.querySelector(".frame").classList.contains("composing-fallback")
    && getComputedStyle(projection).opacity === "1",
  "IME composition keeps the whole-document Markdown projection visible"); assertions += 1;
  const compositionAt = input.selectionStart;
  input.value = `${input.value.slice(0, compositionAt)}漢字${input.value.slice(compositionAt)}`;
  input.setSelectionRange(compositionAt + 2, compositionAt + 2);
  input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertCompositionText", data: "漢字" }));
  const composingLineIndex = input.value.slice(0, compositionAt).split("\n").length - 1;
  check(projection.children[composingLineIndex]?.textContent.includes("漢字")
    && !editor.snapshot().text.includes("漢字"),
  "the composing line previews live text without engine mutation"); assertions += 1;
  check(Boolean(editor.shadowRoot.querySelector(".composition-run"))
    && Boolean(editor.shadowRoot.querySelector(".primary-caret")),
  "the composed run is marked and a projected caret is painted while composing"); assertions += 1;
  check(getComputedStyle(projection.children[0]).fontSize + projection.children[0].className
    === headingStyleBeforeComposition && composingLineIndex !== 0,
  "a distant line keeps its projected style during composition"); assertions += 1;
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "漢字" }));
  check(editor.snapshot().text.includes("漢字"), "composition commits exactly once"); assertions += 1;
  await animationFrames(2);
  check(!editor.shadowRoot.querySelector(".composition-run"),
    "the composed-run marker clears after the commit render"); assertions += 1;

  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  const fallbackAt = input.selectionStart;
  input.value = `${input.value.slice(0, fallbackAt)}multi\nrow${input.value.slice(fallbackAt)}`;
  input.setSelectionRange(fallbackAt + 9, fallbackAt + 9);
  input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertCompositionText", data: "multi\nrow" }));
  check(editor.shadowRoot.querySelector(".frame").classList.contains("composing-fallback")
    && getComputedStyle(input).caretColor !== "rgba(0, 0, 0, 0)"
    && getComputedStyle(input, "::selection").backgroundColor !== "rgba(0, 0, 0, 0)",
  "an unmappable composed run falls back to the frame-wide native reveal"); assertions += 1;
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "multi\nrow" }));
  check(!editor.shadowRoot.querySelector(".frame").classList.contains("composing-fallback"),
    "the native-reveal fallback clears on composition commit"); assertions += 1;
  await animationFrames(2);
  return assertions;
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => count-- <= 0 ? resolve() : requestAnimationFrame(next);
    requestAnimationFrame(next);
  });
}
