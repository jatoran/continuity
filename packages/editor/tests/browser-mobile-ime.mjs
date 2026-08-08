import { synchronizeControlledSnapshot } from "/node_modules/@continuity-editor/editor/src/controlled.js";

// Mobile IME lane (Android GBoard, iOS Safari). A predictive keyboard composes
// a word, then a host document replacement (the controlled-echo reconcile) can
// land mid-composition. Rewriting the textarea under a live composition made the
// keyboard re-inject and compound the document on the next space. The component
// must defer every host replacement until compositionend and drop the stale
// echo so the committed word is never duplicated. Assertions use unique markers
// and occurrence counts so they are robust to trailing-newline normalization.
export async function runMobileImeTests(editor, input, check) {
  let assertions = 0;
  const occurrences = (haystack, needle) => haystack.split(needle).length - 1;
  const frame = input.getRootNode().querySelector(".frame");
  const projection = input.getRootNode().querySelector("[part=projection]");
  let changesDuringComposition = 0;
  let changesAfterCommit = 0;
  const countChange = () => { changesDuringComposition += 1; changesAfterCommit += 1; };
  editor.addEventListener("continuity-change", countChange);

  check(input.getAttribute("autocorrect") === "off",
    "mobile autocorrect is disabled on the semantic input"); assertions += 1;

  // Cycle 1: a host replaceValue that lands during a live composition must defer
  // and must not rewrite the textarea; its stale echo is dropped at commit.
  input.setSelectionRange(input.value.length, input.value.length);
  const baseRevision = editor.snapshot().revision;
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  check(editor.composing === true,
    "composing getter is true during an active IME composition"); assertions += 1;

  const composeAt = input.selectionStart;
  input.value = `${input.value.slice(0, composeAt)}ZZWORDA${input.value.slice(composeAt)}`;
  input.setSelectionRange(composeAt + 7, composeAt + 7);
  input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertCompositionText", data: "ZZWORDA" }));
  const textareaDuringComposition = input.value;
  // The line-scoped preview: every word a mobile keyboard composes must leave
  // the rest of the document's rendered Markdown untouched — no frame-wide
  // raw-source reveal, no whole-document re-wrap.
  check(getComputedStyle(projection).opacity === "1"
    && !frame.classList.contains("composing-fallback"),
  "the projection stays visible while a mobile word composes"); assertions += 1;
  const composingLineIndex = input.value.slice(0, composeAt).split("\n").length - 1;
  check(projection.children[composingLineIndex]?.textContent.includes("ZZWORDA"),
    "the composing line previews the in-progress word"); assertions += 1;
  check(changesDuringComposition === 0,
    "no continuity-change is emitted mid-composition"); assertions += 1;
  const deferred = editor.replaceValue(`${input.value} ZZSTALE`, baseRevision);
  check(deferred === null, "a host replaceValue is deferred while composing"); assertions += 1;
  check(input.value === textareaDuringComposition,
    "the textarea is never rewritten under a live composition"); assertions += 1;
  check(!editor.snapshot().text.includes("ZZWORDA"),
    "the engine has not yet absorbed the composing text"); assertions += 1;

  changesAfterCommit = 0;
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "ZZWORDA" }));
  check(changesAfterCommit === 1,
    "the composition commit emits exactly one change event"); assertions += 1;
  editor.removeEventListener("continuity-change", countChange);
  const afterFirst = editor.snapshot().text;
  check(occurrences(afterFirst, "ZZWORDA") === 1 && !afterFirst.includes("ZZSTALE"),
    `composition commits once and drops the stale host echo (tail=${JSON.stringify(afterFirst.slice(-48))})`);
  assertions += 1;

  // Cycle 2: echoing the prior commit back mid-composition must not multiply it
  // (the reported "swe-muxswe-mux..." escalation on the space key), and the
  // committed word must not double on the textarea round-trip.
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  const secondAt = input.selectionStart;
  input.value = `${input.value.slice(0, secondAt)}ZZWORDB${input.value.slice(secondAt)}`;
  input.setSelectionRange(secondAt + 7, secondAt + 7);
  input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertCompositionText", data: "ZZWORDB" }));
  editor.replaceValue(afterFirst, editor.snapshot().revision);
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "ZZWORDB" }));
  const afterSecond = editor.snapshot().text;
  check(occurrences(afterSecond, "ZZWORDA") === 1 && occurrences(afterSecond, "ZZWORDB") === 1,
    `successive composed words do not compound the document (A=${occurrences(afterSecond, "ZZWORDA")}, B=${occurrences(afterSecond, "ZZWORDB")})`);
  assertions += 1;

  // Adapter guard: controlled reconciliation is suppressed during composition.
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  const suppressed = await synchronizeControlledSnapshot(editor, "ZZDIFFERENT", editor.snapshot().revision);
  check(suppressed === null, "controlled reconciliation is suppressed while composing"); assertions += 1;
  check(!input.value.includes("ZZDIFFERENT"),
    "controlled reconcile does not rewrite the composing textarea"); assertions += 1;
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "" }));

  // Cycle 3: a mid-document composition commit must keep the caret at the
  // composed word instead of relocating it to the document end (the reported
  // mobile "caret jumps to end of file when typing starts" bug).
  const midpoint = input.value.lastIndexOf("\n", Math.floor(input.value.length / 2)) + 1;
  input.setSelectionRange(midpoint, midpoint);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  const midAt = input.selectionStart;
  input.value = `${input.value.slice(0, midAt)}ZZMID${input.value.slice(midAt)}`;
  input.setSelectionRange(midAt + 5, midAt + 5);
  input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertCompositionText", data: "ZZMID" }));
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "ZZMID" }));
  check(input.selectionStart === midAt + 5,
    `a mid-document composition commit keeps the caret at the composed word (caret=${input.selectionStart}, expected=${midAt + 5}, end=${input.value.length})`);
  assertions += 1;
  check(occurrences(editor.snapshot().text, "ZZMID") === 1,
    "a mid-document composition commits exactly once"); assertions += 1;

  // Autocorrect-style native replacement: `insertReplacementText` bypasses the
  // engine beforeinput routing, so the input-event reconcile must adopt the
  // textarea's text and caret without re-applying the splice (re-application
  // used to duplicate the replaced run and throw the caret to the end).
  const fixAt = input.value.indexOf("ZZWORDB");
  input.value = `${input.value.slice(0, fixAt)}ZZFIXED${input.value.slice(fixAt + 7)}`;
  input.setSelectionRange(fixAt + 7, fixAt + 7);
  input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertReplacementText", data: "ZZFIXED" }));
  check(editor.snapshot().text === input.value && occurrences(input.value, "ZZFIXED") === 1,
    "a native replacement reconciles once without duplication"); assertions += 1;
  check(input.selectionStart === fixAt + 7,
    `a native replacement keeps the caret at the replacement (caret=${input.selectionStart}, expected=${fixAt + 7})`);
  assertions += 1;
  return assertions;
}
