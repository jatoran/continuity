import {
  ContinuityEditorElement,
  initialize,
} from "./node_modules/@continuity-editor/editor/index.js";

const host = window.continuityHost;
if (!host) {
  throw new Error("Continuity preload bridge is unavailable");
}
const [initial, wasm] = await Promise.all([host.load(), host.wasm()]);
await initialize({ wasm });

const editorHost = document.querySelector("#editor-host");
const focusButton = document.querySelector("#focus");
const readOnlyButton = document.querySelector("#readonly");
const teardownButton = document.querySelector("#teardown");
const status = document.querySelector("#status");
let editor;
let isReadOnly = false;
let currentSnapshot = { text: initial.text, revision: initial.revision };
let latestPersistence = Promise.resolve();

async function mountEditor() {
  const nextEditor = new ContinuityEditorElement();
  nextEditor.value = currentSnapshot.text;
  nextEditor.initialRevision = currentSnapshot.revision;
  nextEditor.readOnly = isReadOnly;
  nextEditor.shortcutPolicy = "editor-first";
  nextEditor.setAttribute("aria-label", "Electron markdown document");
  nextEditor.addEventListener("continuity-change", (event) => {
    currentSnapshot = event.detail.snapshot;
    if (event.detail.commitOrigin === "host") {
      return;
    }
    latestPersistence = latestPersistence
      .catch(() => undefined)
      .then(() => host.persist(event.detail));
    latestPersistence.then((result) => {
      status.textContent = `Saved revision ${result.revision}; host sequence ${result.sequence}.`;
    }).catch((error) => {
      status.textContent = `Persistence failed: ${error}`;
    });
  });
  nextEditor.addEventListener("focusin", () => host.setEditorFocused(true));
  nextEditor.addEventListener("focusout", () => queueMicrotask(() => {
    host.setEditorFocused(nextEditor.matches(":focus-within"));
  }));
  editorHost.replaceChildren(nextEditor);
  editor = nextEditor;
  await editor.ready;
  focusButton.disabled = false;
  readOnlyButton.disabled = false;
  teardownButton.textContent = "Destroy editor";
  status.textContent = isReadOnly ? "Editor ready and read-only." : "Editor ready and editable.";
}

async function verifyEditorContract() {
  const probe = new ContinuityEditorElement();
  probe.value = "keyboard probe";
  probe.setAttribute("aria-label", "Electron keyboard probe");
  probe.style.cssText = "position:fixed;left:-10000px;width:320px;height:100px";
  document.body.append(probe);
  await probe.ready;
  const input = probe.shadowRoot.querySelector("textarea");
  input.setSelectionRange(4, 4);
  const tabHandled = !input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
  }));
  const shiftTabHandled = !input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
    shiftKey: true,
  }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  const escapeTabReleased = input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
  }));
  const textRestored = probe.snapshot().text === "keyboard probe";
  probe.value = Array.from({ length: 60 }, (_, index) => `follow line ${index}`).join("\n");
  await adoptSelection(probe, input, input.value.length);
  input.scrollTop = 0;
  input.dispatchEvent(new Event("scroll"));
  await insertAndWait(probe, input, "electron-input");
  const projectionTransform = probe.shadowRoot.querySelector(".projection").style.transform;
  const caretFollows = input.scrollTop > 0
    && projectionTransform === `translate(0px, ${-input.scrollTop}px)`;
  await adoptSelection(probe, input, 0);
  input.scrollTop = input.scrollHeight;
  input.dispatchEvent(new Event("scroll"));
  await insertAndWait(probe, input, "^");
  const caretFollowsUpward = input.scrollTop === 0;
  probe.value = "alpha\nbeta\ngamma";
  input.focus();
  input.setSelectionRange(2, 2);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await host.smokeMultiCursor();
  await waitFor(() => probe.snapshot().selections.length === 2);
  const active = probe.snapshot().selections[0]?.head;
  const multiCursorWorks = active?.line === 1 && active?.byteInLine === 2
    && input.selectionStart === 8
    && probe.shadowRoot.querySelectorAll(".secondary-caret").length === 1;
  probe.destroy();
  probe.remove();
  if (
    !tabHandled || !shiftTabHandled || !escapeTabReleased || !textRestored
    || !caretFollows || !caretFollowsUpward || !multiCursorWorks
  ) {
    const detail = JSON.stringify({
      tabHandled,
      shiftTabHandled,
      escapeTabReleased,
      textRestored,
      caretFollows,
      caretFollowsUpward,
      multiCursorWorks,
      active,
      selectionStart: input.selectionStart,
      visibleSecondaryCarets: probe.shadowRoot.querySelectorAll(".secondary-caret").length,
    });
    status.textContent = `Electron editor contract failed: ${detail}`;
    throw new Error(`Electron editor contract failed: ${detail}`);
  }
}

function destroyEditor() {
  if (!editor) {
    return;
  }
  currentSnapshot = editor.snapshot();
  host.setEditorFocused(false);
  editor.destroy();
  editor.remove();
  editor = undefined;
  focusButton.disabled = true;
  readOnlyButton.disabled = true;
  teardownButton.textContent = "Recreate editor";
  status.textContent = "Editor destroyed. Tab should not find an editor field.";
  teardownButton.focus();
}

focusButton.addEventListener("click", () => editor?.focus());
readOnlyButton.addEventListener("click", () => {
  if (!editor) {
    return;
  }
  isReadOnly = !isReadOnly;
  editor.readOnly = isReadOnly;
  status.textContent = isReadOnly ? "Editor is read-only." : "Editor is editable.";
});
teardownButton.addEventListener("click", async () => {
  if (editor) {
    destroyEditor();
  } else {
    await mountEditor();
  }
});

await mountEditor();
host.onEditorCommand((command) => editor?.executeCommand(command));

if (host.isSmoke) {
  await verifyEditorContract();
  destroyEditor();
  if (editorHost.querySelector("continuity-editor")) {
    throw new Error("destroyed Electron editor remained in the document");
  }
  await mountEditor();
  editor.focus();
  const shortcutRevision = editor.snapshot().revision;
  const shortcutText = editor.snapshot().text;
  await host.smokeShortcut();
  await waitFor(() => editor.snapshot().revision > shortcutRevision);
  if (editor.snapshot().text === shortcutText) {
    throw new Error("Electron main-process shortcut interception did not execute Ctrl+E");
  }
  const input = editor.shadowRoot.querySelector("textarea");
  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data: "electron-smoke",
    inputType: "insertText",
  }));
  await latestPersistence;
  const snapshot = editor.snapshot();
  host.onSmokeAck((result) => {
    if (!result.ok) {
      document.body.dataset.smoke = "failed";
    }
  });
  host.smokeComplete(snapshot);
}

async function waitFor(predicate) {
  const deadline = performance.now() + 2_000;
  while (performance.now() < deadline) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out waiting for Electron shortcut dispatch");
}

async function adoptSelection(editorElement, input, offset) {
  const frame = once(editorElement, "continuity-frame");
  input.setSelectionRange(offset, offset);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await frame;
}

async function insertAndWait(editorElement, input, data) {
  const frame = once(editorElement, "continuity-frame");
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data,
    inputType: "insertText",
  }));
  await frame;
}

function once(target, type) {
  return new Promise((resolve) => target.addEventListener(type, resolve, { once: true }));
}
