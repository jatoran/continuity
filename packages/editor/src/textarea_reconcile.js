import { selectionsWithInputPrimary } from "./multi_selection.js";

/**
 * Reconcile a textarea-first mutation (IME composition commit or a native
 * input event such as autocorrect's `insertReplacementText`) into the engine.
 *
 * The textarea already holds both the mutated text and the user's live caret,
 * so the engine adopts them: the text lands as a minimal engine splice and the
 * input selection replaces the engine primary. Callers must sync without
 * re-applying the returned change's edits (the textarea has them) and must not
 * reassert the engine selection over the input. Returns the engine change, or
 * `null` when the textarea already matches the engine text.
 */
export function reconcileTextareaMutation(editor, input) {
  const snapshot = editor.snapshot();
  if (input.value === snapshot.text) {
    return null;
  }
  const change = editor.replaceValue(input.value, snapshot.revision);
  if (change) {
    editor.setSelections(selectionsWithInputPrimary(editor.snapshot(), input));
  }
  return change;
}
