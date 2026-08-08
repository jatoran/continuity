import {
  ContinuityEditorElement,
  ContinuityRendererElement,
  RevisionConflictError,
  initialize,
  listShortcutBindings,
} from "/node_modules/@continuity-editor/editor/index.js";
import { measureTextareaCaretTop } from "/node_modules/@continuity-editor/editor/src/projection_measure.js";
import { runBrowserNiceties } from "./browser-niceties.mjs";
import { runCommandRailTests } from "./browser-command-rail.mjs";
import { runRailExtensionTests } from "./browser-rail-extensions.mjs";
import { runTouchPointerTests } from "./browser-touch-pointer.mjs";
import { runTouchSelectionTests } from "./browser-touch-selection.mjs";
import { runTouchScrollTests } from "./browser-touch-scroll.mjs";
import { runSoftKeyboardGateTests } from "./browser-soft-keyboard.mjs";
import { runCoarsePointerPass } from "./browser-coarse-pass.mjs";
import { runProjectionChromeTests } from "./browser-projection-chrome.mjs";
import { runCompositionPointerTests } from "./browser-composition-pointer.mjs";
import { runViewportPersistenceTests } from "./browser-viewport-persistence.mjs";
import { runCompositionTests } from "./browser-composition.mjs";
import { runMobileImeTests } from "./browser-mobile-ime.mjs";
import { runListNewlineTests } from "./browser-list-newline.mjs";
import { runViewportLineTests } from "./browser-viewport-lines.mjs";
import { runRangeRevealTests } from "./browser-range-reveal.mjs";
import { runPerformanceTests } from "./browser-performance.mjs";
import { runReactBrowserContract } from "./react-browser.mjs";
import { runContinuityConformance } from "/node_modules/@continuity-editor/editor/conformance.js";

const result = document.querySelector("#result");

// The touch surface only exists for a coarse primary pointer, which the runner
// turns on for a dedicated pass after the main contract finishes.
window.__runTouchShieldSuite = () => runCoarsePointerPass(ContinuityEditorElement);

try {
  const initializationStarted = performance.now();
  const wasm = await fetch("/node_modules/@continuity-editor/editor/internal/continuity_wasm_bg.wasm")
    .then((response) => response.arrayBuffer());
  await initialize({ wasm });
  const initializationMs = performance.now() - initializationStarted;
  const presentationSpike = runPresentationSpike();
  const contract = await runContractTests();
  const performanceMetrics = await runPerformanceTests(check);
  const metrics = {
    browser: navigator.userAgent,
    devicePixelRatio: window.devicePixelRatio,
    initializationMs,
    presentationSpike,
    ...performanceMetrics,
    assertions: contract.assertions,
  };
  result.textContent = JSON.stringify(metrics);
  document.body.dataset.result = "pass";
} catch (error) {
  result.textContent = JSON.stringify({ error: error.stack ?? String(error) });
  document.body.dataset.result = "fail";
}

function runPresentationSpike() {
  const sourceLines = Array.from(
    { length: 2_000 },
    (_, index) => `line ${index} with **Markdown**, emoji 😀, and wrapping text`,
  );
  const source = sourceLines.join("\n");
  const fixture = document.createElement("section");
  fixture.style.cssText = "position:absolute;left:-10000px;width:720px;height:420px;overflow:hidden;font:16px/24px monospace";
  document.body.append(fixture);

  const domStarted = performance.now();
  const contentEditable = document.createElement("div");
  contentEditable.contentEditable = "true";
  const domFragment = document.createDocumentFragment();
  sourceLines.forEach((line) => {
    const element = document.createElement("div");
    element.textContent = line;
    domFragment.append(element);
  });
  contentEditable.append(domFragment);
  fixture.append(contentEditable);
  contentEditable.getBoundingClientRect();
  const contentEditableMs = performance.now() - domStarted;
  fixture.replaceChildren();

  const canvasStarted = performance.now();
  const canvasInput = document.createElement("textarea");
  canvasInput.value = source;
  canvasInput.setAttribute("aria-label", "Canvas candidate semantic input");
  const canvas = document.createElement("canvas");
  canvas.width = 720;
  canvas.height = 420;
  const context = canvas.getContext("2d");
  context.font = "16px monospace";
  sourceLines.slice(0, 18).forEach((line, index) => context.fillText(line, 0, 20 + index * 24));
  fixture.append(canvas, canvasInput);
  canvas.getBoundingClientRect();
  const canvasSemanticInputMs = performance.now() - canvasStarted;
  fixture.replaceChildren();

  const hybridStarted = performance.now();
  const hybridInput = document.createElement("textarea");
  hybridInput.value = source;
  hybridInput.setAttribute("aria-label", "Hybrid candidate semantic input");
  const viewport = document.createElement("div");
  const viewportFragment = document.createDocumentFragment();
  sourceLines.slice(0, 24).forEach((line) => {
    const element = document.createElement("div");
    element.textContent = line.replaceAll("**", "");
    viewportFragment.append(element);
  });
  viewport.append(viewportFragment);
  fixture.append(viewport, hybridInput);
  viewport.getBoundingClientRect();
  const hybridDomMs = performance.now() - hybridStarted;
  fixture.remove();
  return {
    lineCount: sourceLines.length,
    contentEditableMs,
    canvasSemanticInputMs,
    hybridDomMs,
    decision: "hybrid-dom-projection-semantic-textarea",
  };
}

async function runContractTests() {
  let assertions = 0;
  assertions += await runReactBrowserContract(check);
  assertions += await runBrowserNiceties(ContinuityEditorElement, check);
  assertions += await runCommandRailTests(ContinuityEditorElement, check);
  assertions += await runRailExtensionTests(ContinuityEditorElement, check);
  assertions += await runTouchPointerTests(ContinuityEditorElement, check);
  assertions += await runTouchSelectionTests(ContinuityEditorElement, check);
  assertions += await runTouchScrollTests(ContinuityEditorElement, check);
  assertions += await runSoftKeyboardGateTests(ContinuityEditorElement, check);
  assertions += await runProjectionChromeTests(ContinuityEditorElement, check);
  assertions += await runCompositionPointerTests(ContinuityEditorElement, check);
  assertions += await runListNewlineTests(ContinuityEditorElement, check);
  assertions += await runViewportLineTests(ContinuityEditorElement, check);
  assertions += await runViewportPersistenceTests(ContinuityEditorElement, check);
  assertions += await runRangeRevealTests(ContinuityEditorElement, check);
  const editor = new ContinuityEditorElement();
  editor.value = "# Heading\n\n[link](https://example.test) café 😀 אבג\n";
  editor.setAttribute("aria-label", "Contract document");
  editor.style.setProperty("--continuity-font-family", '"Courier New", monospace');
  editor.style.setProperty("--continuity-font-size", "19px");
  document.querySelector("#mount").append(editor);
  await editor.ready;
  const input = editor.shadowRoot.querySelector("textarea");
  const projection = editor.shadowRoot.querySelector("[part=projection]");

  check(input instanceof HTMLTextAreaElement, "semantic input is a textarea"); assertions += 1;
  check(input.getAttribute("aria-label") === "Contract document", "accessible name propagates"); assertions += 1;
  check(input.getAttribute("aria-multiline") === "true", "multiline semantics exist"); assertions += 1;
  check(projection.getAttribute("aria-hidden") === "true", "visual projection is hidden from accessibility"); assertions += 1;
  check(projection.children.length === editor.snapshot().text.split("\n").length, "display lines are consumed"); assertions += 1;
  check(projection.children[0].textContent.startsWith("#"), "active line reveals Markdown source markers"); assertions += 1;
  check(getComputedStyle(input).fontFamily.includes("Courier New"), "host font family crosses the shadow boundary"); assertions += 1;
  check(getComputedStyle(input).fontSize === "19px", "host font size crosses the shadow boundary"); assertions += 1;
  const hostChange = once(editor, "continuity-change");
  editor.value = `${editor.value}host`;
  check((await hostChange).detail.commitOrigin === "host", "host replacements carry a non-persistable origin"); assertions += 1;
  await new Promise((resolve) => setTimeout(resolve, 300));
  editor.setSelections([{
    anchor: { line: 0, byteInLine: 0 }, head: { line: 0, byteInLine: 0 }, kind: "caret",
  }], { reveal: false });
  check(editor.snapshot().selections[0].head.line === 0, "programmatic selection reaches the engine"); assertions += 1;
  const scrollState = editor.getScrollState();
  editor.restoreScrollState(scrollState);
  check(editor.linearMemoryBytes() > 0, "element exposes WASM memory telemetry"); assertions += 1;

  const plainEditor = new ContinuityEditorElement();
  plainEditor.syntax = "plain";
  plainEditor.value = "# plain **source**";
  document.body.append(plainEditor);
  await plainEditor.ready;
  check(plainEditor.shadowRoot.querySelector(".projection").textContent.includes("**source**"),
    "plain syntax bypasses Markdown projection"); assertions += 1;
  plainEditor.destroy();
  plainEditor.remove();

  const renderer = new ContinuityRendererElement();
  renderer.value = "# Static heading";
  renderer.style.cssText = "display:block;width:420px;height:120px";
  document.body.append(renderer);
  await renderer.ready;
  check(!renderer.shadowRoot.querySelector("textarea"), "static renderer has no editing textarea"); assertions += 1;
  check(renderer.shadowRoot.querySelector(".projection").textContent.includes("Static heading"),
    "static renderer shares Markdown presentation"); assertions += 1;
  renderer.destroy();
  renderer.remove();
  check((await runContinuityConformance()).passed, "published conformance harness passes"); assertions += 1;

  const taskBinding = listShortcutBindings().find((binding) => binding.command === "markdown.toggle_task");
  check(taskBinding?.chord === "mod+e", "shortcut inventory exposes the default chord"); assertions += 1;
  check(taskBinding?.isBrowserSafe === false, "shortcut inventory exposes browser-safety classification"); assertions += 1;

  const linkLineOffset = input.value.indexOf("[link]");
  input.setSelectionRange(linkLineOffset, linkLineOffset);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  await new Promise((resolve) => setTimeout(resolve, 150));
  check(projection.children[0].textContent === "Heading", "inactive heading hides its source marker"); assertions += 1;
  check(projection.children[2].textContent.startsWith("[link]"), "new active line reveals link markers"); assertions += 1;

  const restoredEditor = new ContinuityEditorElement();
  restoredEditor.value = "restored";
  restoredEditor.initialRevision = 9;
  document.body.append(restoredEditor);
  await restoredEditor.ready;
  check(restoredEditor.snapshot().revision === 9, "host snapshot revision is restored"); assertions += 1;
  const restoredInput = restoredEditor.shadowRoot.querySelector("textarea");
  restoredInput.setSelectionRange(restoredInput.value.length, restoredInput.value.length);
  await insertBeforeInput(restoredEditor, restoredInput, "!");
  check(restoredEditor.snapshot().revision === 10, "restored revision advances monotonically"); assertions += 1;
  restoredEditor.destroy();
  restoredEditor.remove();

  input.focus();
  check(editor.shadowRoot.activeElement === input, "focus delegates to semantic input"); assertions += 1;
  const textBeforeTab = editor.snapshot().text;
  input.setSelectionRange(2, 2);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  const tabEvent = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  check(!input.dispatchEvent(tabEvent), "Tab is owned by the editor by default"); assertions += 1;
  check(editor.snapshot().text.startsWith("\t# Heading"), "Tab indents the covered line through the engine"); assertions += 1;
  const shiftTabEvent = new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
    shiftKey: true,
  });
  check(!input.dispatchEvent(shiftTabEvent), "Shift+Tab is owned by the editor"); assertions += 1;
  check(editor.snapshot().text === textBeforeTab, "Shift+Tab outdents through the engine"); assertions += 1;
  const keyboardHelp = editor.shadowRoot.querySelector("#continuity-keyboard-help");
  check(keyboardHelp.textContent.includes("Escape"), "focus escape is described to assistive technology"); assertions += 1;
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Escape",
    bubbles: true,
    cancelable: true,
  })), "Escape arms focus traversal without being consumed"); assertions += 1;
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
  })), "Escape then Tab is released for browser focus traversal"); assertions += 1;
  editor.setAttribute("tab-behavior", "focus");
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
  })), "tab-behavior focus opts into direct traversal"); assertions += 1;
  editor.removeAttribute("tab-behavior");

  const textBeforeShortcut = editor.snapshot().text;
  const browserSafeConflict = new KeyboardEvent("keydown", {
    key: "e",
    code: "KeyE",
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
  });
  const suppressionEvent = once(editor, "continuity-shortcut-suppressed");
  check(input.dispatchEvent(browserSafeConflict), "browser-safe releases Chrome Ctrl+E"); assertions += 1;
  const suppression = (await suppressionEvent).detail;
  check(suppression.command === "markdown.toggle_task", "suppression event names the released command"); assertions += 1;
  check(suppression.chord === "mod+e" && suppression.policy === "browser-safe", "suppression event explains the policy decision"); assertions += 1;
  check(editor.snapshot().text === textBeforeShortcut, "released browser shortcut does not mutate"); assertions += 1;
  editor.shortcutPolicy = "editor-first";
  input.setSelectionRange(0, 0);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  check(!input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "e",
    code: "KeyE",
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
  })), "editor-first owns a delivered Ctrl+E"); assertions += 1;
  check(editor.snapshot().text.startsWith("- [ ] "), "editor-first command uses the Rust task planner"); assertions += 1;
  editor.executeCommand("markdown.toggle_task");
  check(editor.snapshot().text === textBeforeShortcut, "public executeCommand uses the same operation"); assertions += 1;
  editor.shortcutPolicy = "browser-safe";
  editor.shortcutBindings = { "Mod+E": "markdown.toggle_task" };
  check(!input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "e",
    code: "KeyE",
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
  })), "explicit host binding overrides browser-safe policy"); assertions += 1;
  editor.executeCommand("markdown.toggle_task");
  editor.shortcutBindings = { "Mod+E": null };
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "e",
    code: "KeyE",
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
  })), "null host binding explicitly releases a chord"); assertions += 1;
  editor.shortcutBindings = {};
  editor.shortcutPolicy = "none";
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "b",
    code: "KeyB",
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
  })), "none policy releases default editor commands"); assertions += 1;
  editor.shortcutPolicy = "browser-safe";
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "b",
    code: "KeyB",
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
    altKey: true,
  })), "AltGraph-like input is not mistaken for an editor shortcut"); assertions += 1;

  input.setSelectionRange(input.value.length, input.value.length);
  await insertBeforeInput(editor, input, "日本語");
  check(editor.snapshot().text.endsWith("日本語"), "CJK beforeinput commits"); assertions += 1;
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Dead", bubbles: true }));
  await insertBeforeInput(editor, input, "é");
  check(editor.snapshot().text.endsWith("日本語é"), "dead-key composed scalar survives"); assertions += 1;
  await insertBeforeInput(editor, input, "e\u0301");
  check(editor.snapshot().text.endsWith("日本語ée\u0301"), "combining sequence survives"); assertions += 1;
  await insertBeforeInput(editor, input, "😀");
  check(editor.snapshot().text.endsWith("日本語ée\u0301😀"), "emoji surrogate pair survives"); assertions += 1;
  await insertBeforeInput(editor, input, " אבג");
  check(editor.snapshot().text.endsWith("日本語ée\u0301😀 אבג"), "bidi text survives"); assertions += 1;

  assertions += await runCompositionTests(editor, input, projection, check);
  assertions += await runMobileImeTests(editor, input, check);

  const pasteAt = input.selectionStart;
  input.dispatchEvent(clipboardEvent("paste", " pasted"));
  check(editor.snapshot().text.slice(pasteAt).startsWith(" pasted"), "plain-text paste uses engine mutation"); assertions += 1;
  input.setSelectionRange(pasteAt, pasteAt + 7);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(clipboardEvent("cut", ""));
  check(!editor.snapshot().text.includes(" pasted"), "cut updates canonical text"); assertions += 1;

  input.setSelectionRange(2, 9);
  input.dispatchEvent(new Event("select", { bubbles: true }));
  input.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, clientX: 20, clientY: 20 }));
  input.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, clientX: 20, clientY: 20 }));
  check(editor.snapshot().selections[0].head.byteInLine === 9, "pointer-owned selection publishes UTF-8 coordinates"); assertions += 1;

  let request;
  editor.addEventListener("continuity-request", (event) => { request = event.detail; }, { once: true });
  const linkOffset = input.value.indexOf("link") + 1;
  input.setSelectionRange(linkOffset, linkOffset);
  input.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
  check(request?.kind === "openLink" && request.href === "https://example.test", "link activation is host-mediated"); assertions += 1;

  input.setSelectionRange(input.value.length, input.value.length);
  input.dispatchEvent(dataTransferEvent("drop", { text: " dropped" }));
  check(editor.snapshot().text.endsWith(" dropped"), "plain-text drop uses engine mutation"); assertions += 1;
  let droppedFiles;
  editor.addEventListener("continuity-request", (event) => { droppedFiles = event.detail; }, { once: true });
  input.dispatchEvent(dataTransferEvent("drop", {
    files: [new File(["payload"], "note.md", { type: "text/markdown" })],
  }));
  check(droppedFiles?.kind === "filesDropped" && droppedFiles.files[0].name === "note.md", "file drop is host-mediated"); assertions += 1;
  let contextRequest;
  editor.addEventListener("continuity-request", (event) => { contextRequest = event.detail; }, { once: true });
  input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 17, clientY: 23 }));
  check(contextRequest?.kind === "contextMenu" && contextRequest.clientX === 17, "context menu is host-mediated"); assertions += 1;

  const revision = editor.snapshot().revision;
  const replacement = `${editor.snapshot().text}\nreplacement`;
  editor.replaceValue(replacement, revision);
  check(editor.snapshot().text === replacement, "revision-checked replacement succeeds"); assertions += 1;
  let conflict;
  try {
    editor.replaceValue("stale", revision);
  } catch (error) {
    conflict = error;
  }
  check(conflict instanceof RevisionConflictError, "stale replacement is rejected"); assertions += 1;

  editor.readOnly = true;
  const beforeReadOnly = editor.snapshot().text;
  const readOnlyEvent = new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data: "blocked",
    inputType: "insertText",
  });
  check(!input.dispatchEvent(readOnlyEvent), "read-only input is canceled"); assertions += 1;
  check(editor.snapshot().text === beforeReadOnly, "read-only input cannot mutate"); assertions += 1;
  check(input.getAttribute("aria-readonly") === "true", "read-only state is semantic"); assertions += 1;
  check(editor.snapshot().isReadOnly === true, "snapshot reports component read-only state"); assertions += 1;
  check(input.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Tab",
    bubbles: true,
    cancelable: true,
  })), "read-only Tab is released for focus traversal"); assertions += 1;
  editor.readOnly = false;

  editor.style.width = "480px";
  editor.style.zoom = "1.25";
  await animationFrames(2);
  check(parseFloat(editor.shadowRoot.querySelector(".frame").style.getPropertyValue("--continuity-character-width")) > 0, "browser width measurement runs after resize/zoom"); assertions += 1;
  editor.setAttribute("theme", "light");
  check(getComputedStyle(editor).colorScheme === "light", "explicit theme updates browser color scheme"); assertions += 1;
  check(matchMedia("(prefers-reduced-motion: reduce)").matches, "reduced-motion media preference is exercised"); assertions += 1;

  const destroyed = new ContinuityEditorElement();
  destroyed.value = "teardown";
  document.body.append(destroyed);
  await destroyed.ready;
  let destroyEvent = false;
  destroyed.addEventListener("continuity-destroy", () => { destroyEvent = true; });
  destroyed.remove();
  await Promise.resolve();
  await Promise.resolve();
  check(destroyEvent, "disconnect tears down resources"); assertions += 1;
  let destroyedThrows = false;
  try { destroyed.snapshot(); } catch { destroyedThrows = true; }
  check(destroyedThrows, "destroyed element rejects use"); assertions += 1;

  const destroyedBeforeReady = new ContinuityEditorElement();
  const rejectedReady = destroyedBeforeReady.ready.then(
    () => false,
    (error) => error.message.includes("destroyed before becoming ready"),
  );
  destroyedBeforeReady.destroy();
  check(await rejectedReady, "teardown before connection rejects ready"); assertions += 1;

  editor.destroy();
  editor.remove();
  const accessibilityEditor = new ContinuityEditorElement();
  accessibilityEditor.value = "Accessibility tree fixture";
  accessibilityEditor.setAttribute("aria-label", "Accessibility contract document");
  accessibilityEditor.readOnly = true;
  document.body.append(accessibilityEditor);
  await accessibilityEditor.ready;

  const anchorEditor = new ContinuityEditorElement();
  anchorEditor.value = Array.from(
    { length: 30 },
    (_, index) => `anchor line ${index} ${"wrapping ".repeat(14)}`,
  ).join("\n");
  anchorEditor.setAttribute("aria-label", "Caret anchor document");
  anchorEditor.style.cssText = "display:block;width:520px;height:180px";
  document.body.append(anchorEditor);
  await anchorEditor.ready;
  const anchorInput = anchorEditor.shadowRoot.querySelector("textarea");
  const anchorOffset = anchorInput.value.indexOf("anchor line 18") + 80;
  anchorInput.setSelectionRange(anchorOffset, anchorOffset);
  anchorInput.dispatchEvent(new Event("select", { bubbles: true }));
  await animationFrames(2);
  const caretTopBefore = measureTextareaCaretTop(anchorInput);
  anchorInput.scrollTop = Math.max(0, caretTopBefore - 70);
  anchorInput.dispatchEvent(new Event("scroll"));
  const screenYBefore = caretTopBefore - anchorInput.scrollTop;
  anchorEditor.style.width = "260px";
  await animationFrames(3);
  const screenYAfter = measureTextareaCaretTop(anchorInput) - anchorInput.scrollTop;
  check(
    Math.abs(screenYAfter - screenYBefore) <= 2,
    `soft-wrap resize preserves caret screen y (${screenYBefore} -> ${screenYAfter}, scroll ${anchorInput.scrollTop})`,
  ); assertions += 1;

  const followEditor = new ContinuityEditorElement();
  followEditor.value = Array.from({ length: 60 }, (_, index) => `follow line ${index}`).join("\n");
  followEditor.setAttribute("aria-label", "Caret follow document");
  followEditor.style.cssText = "display:block;width:420px;height:120px";
  document.body.append(followEditor);
  await followEditor.ready;
  const followInput = followEditor.shadowRoot.querySelector("textarea");
  followInput.setSelectionRange(followInput.value.length, followInput.value.length);
  followInput.scrollTop = 0;
  followInput.dispatchEvent(new Event("scroll"));
  await insertBeforeInput(followEditor, followInput, "!");
  const followLineHeight = parseFloat(getComputedStyle(followInput).lineHeight);
  const bottomCaretTop = measureTextareaCaretTop(followInput);
  check(followInput.scrollTop > 0, "typing below the viewport follows the caret"); assertions += 1;
  check(
    bottomCaretTop >= followInput.scrollTop
      && bottomCaretTop + followLineHeight <= followInput.scrollTop + followInput.clientHeight + 1,
    "the followed bottom caret remains fully visible",
  ); assertions += 1;
  check(
    followEditor.shadowRoot.querySelector(".projection").style.transform
      === `translate(0px, ${-followInput.scrollTop}px)`,
    "caret reveal synchronizes the visual projection immediately",
  ); assertions += 1;

  followInput.scrollTop = 0;
  followInput.dispatchEvent(new Event("scroll"));
  await dispatchBeforeInput(followEditor, followInput, "historyUndo");
  check(followInput.scrollTop > 0, "undo follows its restored bottom caret"); assertions += 1;

  followInput.setSelectionRange(0, 0);
  followInput.scrollTop = followInput.scrollHeight;
  followInput.dispatchEvent(new Event("scroll"));
  await insertBeforeInput(followEditor, followInput, "^");
  check(followInput.scrollTop === 0, "typing above the viewport follows the caret upward"); assertions += 1;

  followInput.setSelectionRange(0, followInput.value.length, "backward");
  check(
    measureTextareaCaretTop(followInput) < followInput.clientHeight,
    "caret measurement uses the active head of a backward selection",
  ); assertions += 1;

  followInput.setSelectionRange(followInput.value.length, followInput.value.length);
  followInput.scrollTop = Math.floor(followInput.scrollHeight / 2);
  const hostReplacementScrollTop = followInput.scrollTop;
  followEditor.value = followEditor.value.replace("follow line 20", "hosted line 20");
  check(
    Math.abs(followInput.scrollTop - hostReplacementScrollTop) <= 1,
    "host replacement preserves the host-selected viewport",
  ); assertions += 1;

  const previousButton = document.createElement("button");
  previousButton.id = "pointer-previous";
  previousButton.textContent = "Previous focus target";
  document.body.append(previousButton);
  const pointerEditor = new ContinuityEditorElement();
  pointerEditor.value = "- **alpha** beta gamma delta epsilon zeta eta theta iota kappa lambda TARGET omega\nsecond pointer line";
  pointerEditor.setAttribute("aria-label", "Pointer hit-test document");
  pointerEditor.style.cssText = "display:block;width:320px;height:180px;--continuity-font-family:Georgia,serif;--continuity-font-size:19px";
  document.body.append(pointerEditor);
  await pointerEditor.ready;
  pointerEditor.setSelections([{
    anchor: { line: 1, byteInLine: 0 }, head: { line: 1, byteInLine: 0 }, kind: "caret",
  }], { reveal: false });
  const nextButton = document.createElement("button");
  nextButton.id = "pointer-next";
  nextButton.textContent = "Next focus target";
  document.body.append(nextButton);
  return { assertions };
}

async function insertBeforeInput(editor, input, data) {
  return dispatchBeforeInput(editor, input, "insertText", data);
}

async function dispatchBeforeInput(editor, input, inputType, data = null) {
  const frame = once(editor, "continuity-frame");
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data,
    inputType,
  }));
  await frame;
}

function clipboardEvent(type, text) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  const values = new Map([["text/plain", text]]);
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (format) => values.get(format) ?? "",
      setData: (format, value) => values.set(format, value),
    },
  });
  return event;
}

function dataTransferEvent(type, { text = "", files = [] }) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", {
    value: {
      files,
      getData: (format) => format === "text/plain" ? text : "",
    },
  });
  return event;
}

function once(target, eventName) {
  return new Promise((resolve) => target.addEventListener(eventName, resolve, { once: true }));
}

function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => count-- <= 0 ? resolve() : requestAnimationFrame(next);
    next();
  });
}

function check(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}
