// Input types a pressed Enter raises on a textarea. `insertParagraph` is what
// the same key reports where the browser treats the surface as block content,
// so both spellings mean one thing here: the user asked for a line break.
const LINE_BREAK_INPUT_TYPES = new Set(["insertLineBreak", "insertParagraph"]);

/** Route supported beforeinput mutations through the authoritative engine. */
export function handleBeforeInput(event, options) {
  if (options.isReadOnly) {
    event.preventDefault();
    return;
  }
  if (options.isComposing || event.inputType === "insertCompositionText") {
    // Android keyboards hold a composition open across ordinary typing, so
    // returning here unconditionally is permanent on a phone rather than
    // exceptional: Enter fell through to the textarea's own newline, the
    // list-aware planner never ran, and every split dropped its marker.
    //
    // A line break is not composed text. The IME raises it only because the
    // user pressed Enter, so commit the composing run and let the engine plan
    // the break against text it now matches.
    //
    // `beforeinput` is the discriminating point, not `keydown`: an IME's
    // candidate-commit Enter reports `key: "Enter"` (or `Unidentified`) while
    // composing but raises no line-break `beforeinput` at all, so committing
    // here can never turn that key into a newline.
    if (!LINE_BREAK_INPUT_TYPES.has(event.inputType)) return;
    options.commitComposition();
  }
  const operations = {
    insertText: ["insert", event.data ?? ""],
    insertLineBreak: ["command", "editor.insert_newline_smart"],
    insertParagraph: ["command", "editor.insert_newline_smart"],
    deleteContentBackward: ["deleteBackward"],
    deleteContentForward: ["deleteForward"],
    deleteByCut: ["insert", ""],
    historyUndo: ["undo"],
    historyRedo: ["redo"],
  };
  const operation = operations[event.inputType];
  if (!operation) return;
  event.preventDefault();
  options.applyOperation(operation[0], operation[1]);
}
