import { initialize } from "/node_modules/@continuity-editor/editor/index.js";

const editor = document.querySelector("continuity-editor");
const status = document.querySelector("#status");
const wasm = await fetch("/node_modules/@continuity-editor/editor/internal/continuity_wasm_bg.wasm")
  .then((response) => response.arrayBuffer());
await initialize({ wasm });
editor.value = "# Manual accessibility document\n\nType here with your normal keyboard and IME.\n\n- Navigate by character, word, and line\n- Select and replace text\n- Copy, cut, paste, undo, and redo\n\n[Example link](https://example.test)\n";
await editor.ready;
status.textContent = "Editor ready.";

editor.addEventListener("continuity-change", (event) => {
  status.textContent = `Revision ${event.detail.snapshot.revision}; ${event.detail.snapshot.text.length} characters.`;
});
document.querySelector("#focus").addEventListener("click", () => editor.focus());
document.querySelector("#readonly").addEventListener("click", () => {
  editor.readOnly = !editor.readOnly;
  status.textContent = editor.readOnly ? "Editor is read-only." : "Editor is editable.";
  editor.focus();
});
document.querySelector("#theme").addEventListener("click", () => {
  editor.setAttribute("theme", editor.getAttribute("theme") === "light" ? "dark" : "light");
  status.textContent = `Theme changed to ${editor.getAttribute("theme")}.`;
});
