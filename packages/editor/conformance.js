import { attachContinuityEditor } from "./src/controller.js";
import "./index.js";

/** Exercise the disposable controlled-element contract used by host integration tests. */
export async function runContinuityConformance(options = {}) {
  if (!globalThis.document) throw new Error("Continuity conformance requires a browser document");
  const element = options.createElement?.() ?? document.createElement("continuity-editor");
  const changes = [];
  const persisted = [];
  const controller = attachContinuityEditor(element, {
    value: "# Conformance",
    revision: 0,
    callbacks: {
      onChange(detail) {
        changes.push(detail);
        if (detail.commitOrigin === "user") persisted.push(detail.snapshot.revision);
      },
    },
  });
  document.body.append(element);
  try {
    await element.ready;
    await controller.replaceCurrent("# Host replacement");
    element.setSelections([caretAtDocumentEnd(element.snapshot().text)]);
    element.executeCommand("editor.insert_newline_smart", Date.now());
    const host = changes.find((change) => change.commitOrigin === "host");
    if (!host || persisted.includes(host.snapshot.revision)) {
      throw new Error("hostReplacement origin filtering failed");
    }
    return Object.freeze({ passed: true, assertions: 3, changes: changes.length });
  } finally {
    controller.dispose();
    element.remove();
  }
}

function caretAtDocumentEnd(text) {
  const lines = text.split("\n");
  const position = { line: lines.length - 1, byteInLine: new TextEncoder().encode(lines.at(-1)).length };
  return { anchor: position, head: position, kind: "caret" };
}
