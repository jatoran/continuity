/** Apply ordered engine splices to the component-agent-owned source-line mirror. */
export function applySourceEdits(sourceLines, edits, snapshotText) {
  if (edits.length === 0) return sourceLines;
  if (edits.some((edit) => !Number.isInteger(edit.startUtf16InLine)
    || !Number.isInteger(edit.endUtf16InLine))) {
    return snapshotText.split("\n");
  }
  for (const edit of edits) {
    const firstLine = sourceLines[edit.startLine];
    const lastLine = sourceLines[edit.endLine];
    if (firstLine === undefined || lastLine === undefined) return snapshotText.split("\n");
    const inserted = edit.insertedText.split("\n");
    const prefix = firstLine.slice(0, edit.startUtf16InLine);
    const suffix = lastLine.slice(edit.endUtf16InLine);
    inserted[0] = prefix + inserted[0];
    inserted[inserted.length - 1] += suffix;
    sourceLines.splice(edit.startLine, edit.endLine - edit.startLine + 1, ...inserted);
  }
  return sourceLines;
}
