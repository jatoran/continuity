/** Route a browser paste into one canonical plain-text insertion. */
export function handlePaste(event, isReadOnly, applyText) {
  if (isReadOnly) {
    event.preventDefault();
    return;
  }
  const text = event.clipboardData?.getData("text/plain");
  if (text !== undefined) {
    event.preventDefault();
    applyText(normalizeNewlines(text));
  }
}

/** Populate the browser clipboard, then delete through the shared engine. */
export function handleCut(event, isReadOnly, snapshot, input, deleteSelection) {
  if (isReadOnly) {
    event.preventDefault();
    return;
  }
  const selected = snapshot.selections.length > 1
    ? snapshot.selections
      .map((selection) => selectedText(snapshot.text, selection))
      .filter((selectionText) => selectionText.length > 0)
      .join("\n")
    : input.value.slice(input.selectionStart, input.selectionEnd);
  event.clipboardData?.setData("text/plain", selected);
  event.preventDefault();
  deleteSelection();
}

/** Keep file routing host-owned and plain-text drops engine-owned. */
export function handleDrop(event, isReadOnly, onFiles, applyText) {
  event.preventDefault();
  const files = [...(event.dataTransfer?.files ?? [])];
  if (files.length > 0) {
    onFiles(files);
    return;
  }
  if (!isReadOnly) {
    const text = event.dataTransfer?.getData("text/plain");
    if (text) {
      applyText(normalizeNewlines(text));
    }
  }
}

/** Normalize browser and host text to the engine's LF-only contract. */
export function normalizeNewlines(value) {
  return value.replace(/\r\n?/gu, "\n");
}

function selectedText(text, selection) {
  const anchor = positionToUtf16Offset(text, selection.anchor);
  const head = positionToUtf16Offset(text, selection.head);
  return text.slice(Math.min(anchor, head), Math.max(anchor, head));
}
import { positionToUtf16Offset } from "./coordinates.js";

/** Copy all non-collapsed engine selections in stable order. */
export function handleCopy(event, snapshot) {
  if (snapshot.selections.length <= 1) {
    return;
  }
  const text = snapshot.selections
    .map((selection) => selectedText(snapshot.text, selection))
    .filter((selectionText) => selectionText.length > 0)
    .join("\n");
  event.clipboardData?.setData("text/plain", text);
  event.preventDefault();
}
