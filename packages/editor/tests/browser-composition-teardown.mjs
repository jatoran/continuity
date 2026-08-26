// Composition survival across teardown.
//
// A composing run is withheld from the engine on purpose: `beforeinput` refuses
// it and `input` only paints a preview, so the text exists solely in the
// internal textarea and no `continuity-change` has been emitted for it. Every
// path that ends a composition therefore has to fold it in, and teardown is a
// path that ends a composition — an unmount, a route change, a host that calls
// `destroy()`.
//
// This is not an exotic case. Android keyboards hold one composition open
// across ordinary typing, so the uncommitted run is routinely the last word the
// user typed: unmounting mid-sentence on a phone lost it every time.
//
// These cases mount and destroy their own element rather than sharing the
// suite's, because destroying the shared one would end the suite.

import { settle } from "./browser-touch-helpers.mjs";

const SOURCE = "alpha bravo ";

export async function runCompositionTeardownTests(ContinuityEditorElement, check) {
  let assertions = 0;
  assertions += await runTeardownCommitCase(ContinuityEditorElement, check);
  assertions += await runExplicitCommitCase(ContinuityEditorElement, check);
  assertions += await runNoCompositionCase(ContinuityEditorElement, check);
  assertions += await runUnmountCase(ContinuityEditorElement, check);
  return assertions;
}

/** `destroy()` mid-composition must emit the run rather than discard it. */
async function runTeardownCommitCase(ContinuityEditorElement, check) {
  const { editor, input, dispose } = await mount(ContinuityEditorElement);
  let assertions = 0;
  const changes = [];
  editor.addEventListener("continuity-change", (event) => changes.push(event.detail));

  compose(input, "charlie");
  check(editor.composing === true, "the composition is open"); assertions += 1;
  check(!editor.snapshot().text.includes("charlie"),
    "the engine deliberately withholds the composing run"); assertions += 1;
  check(changes.length === 0, "nothing has been emitted for it yet"); assertions += 1;

  editor.destroy();
  const committed = changes.filter((detail) => detail.source === "compositionCommit");
  check(committed.length === 1,
    `teardown emits exactly one composition commit (got ${JSON.stringify(changes.map((c) => c.source))})`);
  assertions += 1;
  check(committed[0]?.snapshot.text.includes("charlie"),
    `the emitted snapshot carries the composed run (got ${JSON.stringify(committed[0]?.snapshot.text)})`);
  assertions += 1;
  check(committed[0]?.commitOrigin === "user",
    "the teardown commit is the user's text, not a host replacement"); assertions += 1;
  // The whole point: a host persisting on the change stream can rebuild the
  // document from what it received, with nothing left in the textarea.
  check(committed.at(-1)?.snapshot.text === "alpha bravo charlie",
    `the persisted document is complete (got ${JSON.stringify(committed.at(-1)?.snapshot.text)})`);
  assertions += 1;
  check(changes.every((detail) => typeof detail.snapshot.revision === "number"),
    "the teardown change carries a usable revision"); assertions += 1;

  dispose();
  return assertions;
}

/** The public method lets a host checkpoint without tearing anything down. */
async function runExplicitCommitCase(ContinuityEditorElement, check) {
  const { editor, input, dispose } = await mount(ContinuityEditorElement);
  let assertions = 0;
  const changes = [];
  editor.addEventListener("continuity-change", (event) => changes.push(event.detail));

  compose(input, "delta");
  check(typeof editor.commitComposition === "function",
    "commitComposition is part of the public element API"); assertions += 1;
  check(editor.commitComposition() === true,
    "committing an open composition reports that it folded a run"); assertions += 1;
  check(editor.composing === false, "the composition is closed afterwards"); assertions += 1;
  check(editor.snapshot().text === "alpha bravo delta",
    `the engine now holds the composed run (got ${JSON.stringify(editor.snapshot().text)})`);
  assertions += 1;
  check(changes.filter((detail) => detail.source === "compositionCommit").length === 1,
    "the explicit commit emits exactly one change"); assertions += 1;

  // Editing continues normally: the checkpoint is not a teardown.
  await settle();
  input.setSelectionRange(input.value.length, input.value.length);
  editor.insertText("!");
  check(editor.snapshot().text.endsWith("!"), "the editor is still live after a checkpoint");
  assertions += 1;

  dispose();
  return assertions;
}

/** With nothing composing, both paths are inert. */
async function runNoCompositionCase(ContinuityEditorElement, check) {
  const { editor, dispose } = await mount(ContinuityEditorElement);
  let assertions = 0;
  check(editor.commitComposition() === false,
    "committing with no composition open reports no work"); assertions += 1;
  let changeCount = 0;
  editor.addEventListener("continuity-change", () => { changeCount += 1; });
  editor.destroy();
  check(changeCount === 0, "a teardown with nothing composing emits no change"); assertions += 1;
  // Destroyed elements still refuse use, and the new method is not an exception
  // that resurrects them.
  check(editor.commitComposition() === false,
    "committing after teardown is inert rather than throwing"); assertions += 1;
  dispose();
  return assertions;
}

/** The phone case: the run survives an unmount, not just an explicit destroy. */
async function runUnmountCase(ContinuityEditorElement, check) {
  const { editor, input } = await mount(ContinuityEditorElement);
  let assertions = 0;
  const changes = [];
  editor.addEventListener("continuity-change", (event) => changes.push(event.detail));

  compose(input, "echo");
  editor.remove();
  // `disconnectedCallback` defers to a microtask so a move within the DOM is
  // not a teardown; wait past it.
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));

  const committed = changes.filter((detail) => detail.source === "compositionCommit");
  check(committed.length === 1,
    `unmounting mid-composition commits the run (got ${committed.length} commit(s))`);
  assertions += 1;
  check(committed[0]?.snapshot.text === "alpha bravo echo",
    `the unmounted document is complete (got ${JSON.stringify(committed[0]?.snapshot.text)})`);
  assertions += 1;
  return assertions;
}

/**
 * Drive the textarea the way an IME does: open a composition, write the run
 * into the textarea directly, and announce it with an `input` event. The engine
 * must not see the text through any of it.
 */
function compose(input, run) {
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
  const at = input.selectionStart;
  input.value = `${input.value.slice(0, at)}${run}${input.value.slice(at)}`;
  input.setSelectionRange(at + run.length, at + run.length);
  input.dispatchEvent(new InputEvent("input", {
    bubbles: true, inputType: "insertCompositionText", data: run,
  }));
}

async function mount(ContinuityEditorElement) {
  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", "Composition teardown document");
  editor.value = SOURCE;
  document.querySelector("#mount").append(editor);
  await editor.ready;
  await settle();
  return {
    editor,
    input: editor.shadowRoot.querySelector("textarea"),
    dispose: () => { editor.destroy(); editor.remove(); },
  };
}
