import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { startBrowserProfile, stopBrowserProfile } from "./browser-profile.mjs";
import { createStaticServer } from "./browser-static-server.mjs";
import { auditTouchShield } from "./browser-shield-audit.mjs";
import {
  assertArrowCaretRepaint,
  assertProjectedMultiClickSelections,
  assertTaskShortcutCarets,
  assertWrappedSelectionInteractions,
  projectedPointerBounds,
  wrappedVisualCaretAlignment,
} from "./browser-pointer-contract.mjs";

const PERFORMANCE_ATTEMPTS = 2;

async function runBrowserContract() {
  const root = resolve(process.argv[2] ?? ".");
  const server = createStaticServer(root);
  await new Promise((ready) => server.listen(0, "127.0.0.1", ready));
  const address = server.address();
  const url = `http://127.0.0.1:${address.port}/browser.html`;
  const chrome = spawn(discoverChromium(), [
    "--headless=new",
    "--disable-gpu",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-dev-shm-usage",
    "--disable-renderer-backgrounding",
    "--no-first-run",
    "--no-default-browser-check",
    "--remote-debugging-port=0",
    `--user-data-dir=${join(root, ".chrome-profile")}`,
    url,
  ], { stdio: ["ignore", "ignore", "pipe"] });

  try {
    const browserEndpoint = await devtoolsEndpoint(chrome);
    const port = new URL(browserEndpoint).port;
    const target = await waitForPageTarget(port);
    const page = new CdpConnection(target.webSocketDebuggerUrl);
    await page.open();
    await page.send("Runtime.enable");
    await page.send("Page.enable");
    await startBrowserProfile(page);
    await page.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });
    await page.send("Emulation.setDeviceMetricsOverride", {
      width: 1_280,
      height: 800,
      deviceScaleFactor: 1.25,
      mobile: false,
    });
    let result;
    for (let attempt = 1; attempt <= PERFORMANCE_ATTEMPTS; attempt += 1) {
      await evaluate(page, "document.body.dataset.result = 'reloading'");
      await page.send("Page.navigate", { url: `${url}?attempt=${attempt}` });
      const state = await waitForResult(page);
      const payload = await evaluate(page, "document.querySelector('#result').textContent");
      result = JSON.parse(payload || "{}");
      if (state === "pass") {
        break;
      }
      if (attempt === PERFORMANCE_ATTEMPTS || !isPerformanceFailure(result)) {
        throw new Error(`browser contract state=${state}: ${JSON.stringify(result)}\n${page.logs.join("\n")}`);
      }
      process.stderr.write(
        `browser performance attempt ${attempt} failed; retrying the full sample: ${result.error}\n`,
      );
    }
    result.accessibility = await auditAccessibility(page);
    result.caretFollow = await auditCaretFollow(page);
    result.pointerAndKeyboard = await auditPointerAndKeyboard(page);
    result.touchShield = await auditTouchShield(page);
    process.stdout.write(`CONTINUITY_BROWSER_METRICS ${JSON.stringify(result)}\n`);
    const profile = await stopBrowserProfile(page);
    if (profile) process.stdout.write(`CONTINUITY_BROWSER_PROFILE ${JSON.stringify(profile)}\n`);
    await page.close();
    const browser = new CdpConnection(browserEndpoint);
    await browser.open();
    await browser.send("Browser.close");
    await browser.close();
  } finally {
    if (chrome.exitCode === null) {
      chrome.kill();
    }
    await new Promise((closed) => server.close(closed));
  }
}

function isPerformanceFailure(result) {
  const message = String(result?.error ?? "");
  return message.includes("large-document")
    || message.includes("input-to-paint-ready")
    || message.includes("10k-line");
}

class CdpConnection {
  constructor(endpoint) {
    this.endpoint = endpoint;
    this.logs = [];
    this.nextId = 0;
    this.pending = new Map();
  }

  async open() {
    this.socket = new WebSocket(this.endpoint);
    this.socket.addEventListener("message", ({ data }) => this.onMessage(JSON.parse(data)));
    await new Promise((resolveOpen, reject) => {
      this.socket.addEventListener("open", resolveOpen, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
  }

  send(method, params = {}) {
    const id = ++this.nextId;
    this.socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolveSend, reject) => this.pending.set(id, { resolveSend, reject }));
  }

  async close() {
    if (this.socket?.readyState === WebSocket.OPEN) {
      this.socket.close();
    }
  }

  onMessage(message) {
    if (message.id) {
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) {
        pending?.reject(new Error(message.error.message));
      } else {
        pending?.resolveSend(message.result);
      }
      return;
    }
    if (message.method === "Runtime.exceptionThrown") {
      this.logs.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    } else if (message.method === "Runtime.consoleAPICalled") {
      this.logs.push(message.params.args.map((argument) => argument.value ?? argument.description).join(" "));
    }
  }
}

async function waitForResult(page) {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    const state = await evaluate(page, "document.body?.dataset.result");
    if (state === "pass" || state === "fail") {
      return state;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  }
  return "timeout";
}

async function evaluate(page, expression) {
  const response = await page.send("Runtime.evaluate", { expression, returnByValue: true });
  if (response.exceptionDetails) {
    throw new Error(
      response.exceptionDetails.exception?.description ?? response.exceptionDetails.text,
    );
  }
  return response.result.value;
}

async function auditAccessibility(page) {
  const { nodes } = await page.send("Accessibility.getFullAXTree");
  const readOnlyTextbox = nodes.find((node) => (
    node.role?.value === "textbox"
    && node.name?.value === "Accessibility contract document"
  ));
  if (!readOnlyTextbox) {
    throw new Error("accessibility tree does not expose the semantic editor textbox");
  }
  const readOnlyProperties = Object.fromEntries(
    (readOnlyTextbox.properties ?? []).map((property) => [property.name, property.value?.value]),
  );
  if (readOnlyProperties.multiline !== true || readOnlyProperties.readonly !== true) {
    throw new Error(`semantic textbox properties are incomplete: ${JSON.stringify(readOnlyProperties)}`);
  }
  const readOnlyDescription = readOnlyTextbox.description?.value ?? "";
  if (!readOnlyDescription.includes("Tab moves focus")) {
    throw new Error(`read-only textbox does not expose direct focus traversal: ${readOnlyDescription}`);
  }

  const editableTextbox = nodes.find((node) => (
    node.role?.value === "textbox"
    && node.name?.value === "Pointer hit-test document"
  ));
  const editableDescription = editableTextbox?.description?.value ?? "";
  if (!editableDescription.includes("Escape") || !editableDescription.includes("Tab")) {
    throw new Error(`editable textbox does not expose focus-escape instructions: ${editableDescription}`);
  }
  return {
    role: readOnlyTextbox.role.value,
    name: readOnlyTextbox.name.value,
    editableDescription,
    readOnlyDescription,
    multiline: true,
    readonly: true,
  };
}

async function auditPointerAndKeyboard(page) {
  const bounds = await projectedPointerBounds(page, evaluate);
  if (!bounds.targetIsWrapped) throw new Error("projected pointer target did not wrap");
  const y = bounds.targetY;
  await page.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: bounds.targetX,
    y,
    button: "left",
    clickCount: 1,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: bounds.targetX,
    y,
    button: "left",
    clickCount: 1,
  });
  const caret = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]')
    .shadowRoot.querySelector('textarea').selectionStart`);
  if (caret !== bounds.expectedTarget) {
    throw new Error(`projected wrapped-line pointer expected ${bounds.expectedTarget}, got ${caret}`);
  }
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  const visualCaret = await wrappedVisualCaretAlignment(page, evaluate);
  if (!visualCaret.caretExists
      || visualCaret.sourceVisible !== "true"
      || visualCaret.caretLeftDelta > 1
      || visualCaret.caretTopDelta > 1
      || visualCaret.nativeCaretColor !== "rgba(0, 0, 0, 0)") {
    throw new Error(`wrapped visual caret does not match projected glyph: ${JSON.stringify(visualCaret)}`);
  }
  const arrowCaret = await assertArrowCaretRepaint(page, evaluate);
  const multiClickSelections = await assertProjectedMultiClickSelections(page, evaluate, {
    x: bounds.targetX,
    y: bounds.targetY,
  });

  const wrappedSelection = await assertWrappedSelectionInteractions(page, evaluate);

  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const input = host.shadowRoot.querySelector('textarea');
    host.style.removeProperty('--continuity-font-family'); host.style.removeProperty('--continuity-font-size'); host.style.width = '420px';
    host.shortcutPolicy = 'browser-safe';
    input.focus();
    input.setSelectionRange(10, 10);
  })()`);
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "ArrowLeft",
    code: "ArrowLeft",
    modifiers: 10,
    windowsVirtualKeyCode: 37,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "ArrowLeft",
    code: "ArrowLeft",
    modifiers: 10,
    windowsVirtualKeyCode: 37,
  });
  const wordSelectionLength = await evaluate(page, `(() => {
    const input = document.querySelector('[aria-label="Pointer hit-test document"]')
      .shadowRoot.querySelector('textarea');
    return input.selectionEnd - input.selectionStart;
  })()`);
  if (wordSelectionLength <= 0) {
    throw new Error("Ctrl+Shift+Left did not retain native word-selection behavior");
  }

  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const input = host.shadowRoot.querySelector('textarea');
    host.value = 'alpha\\nbeta\\ngamma';
    input.focus();
    input.setSelectionRange(2, 2);
    input.dispatchEvent(new Event('select', { bubbles: true }));
  })()`);
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "ArrowDown",
    code: "ArrowDown",
    modifiers: 3,
    windowsVirtualKeyCode: 40,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "ArrowDown",
    code: "ArrowDown",
    modifiers: 3,
    windowsVirtualKeyCode: 40,
  });
  const verticalCarets = await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    return {
      selections: host.snapshot().selections,
      inputSelectionStart: host.shadowRoot.querySelector('textarea').selectionStart,
      visibleSecondaryCarets: host.shadowRoot.querySelectorAll('.secondary-caret').length,
    };
  })()`);
  if (verticalCarets.selections.length !== 2 || verticalCarets.visibleSecondaryCarets !== 1 || verticalCarets.selections[0]?.head.line !== 1 || verticalCarets.selections[0]?.head.byteInLine !== 2 || verticalCarets.inputSelectionStart !== 8) {
    throw new Error(`real Ctrl+Alt+Down did not create two visible carets: ${JSON.stringify(verticalCarets)}`);
  }
  await page.send("Input.insertText", { text: "!" });
  const multiCaretText = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]').snapshot().text`);
  if (multiCaretText !== "al!pha\nbe!ta\ngamma") {
    throw new Error(`real multi-caret typing missed a selection: ${JSON.stringify(multiCaretText)}`);
  }
  await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape" });
  await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape" });
  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const input = host.shadowRoot.querySelector('textarea');
    host.value = 'alpha beta gamma delta epsilon\\nsecond pointer line';
    input.focus();
    input.setSelectionRange(0, 0);
    input.dispatchEvent(new Event('select', { bubbles: true }));
  })()`);
  await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  await page.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: bounds.left + 105,
    y: bounds.top + 55,
    button: "left",
    modifiers: 2,
    clickCount: 1,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: bounds.left + 105,
    y: bounds.top + 55,
    button: "left",
    modifiers: 2,
    clickCount: 1,
  });
  const pointerCarets = await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    return {
      selections: host.snapshot().selections,
      inputSelectionStart: host.shadowRoot.querySelector('textarea').selectionStart,
      visibleSecondaryCarets: host.shadowRoot.querySelectorAll('.secondary-caret').length,
    };
  })()`);
  if (pointerCarets.selections.length !== 2 || pointerCarets.visibleSecondaryCarets !== 1 || pointerCarets.selections[0]?.head.line !== 1 || pointerCarets.inputSelectionStart < 31) {
    throw new Error(`real Ctrl+click did not create two visible carets: ${JSON.stringify(pointerCarets)}`);
  }

  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const input = host.shadowRoot.querySelector('textarea');
    host.shortcutPolicy = 'editor-first';
    input.focus();
    input.setSelectionRange(0, 0);
    input.dispatchEvent(new Event('select', { bubbles: true }));
  })()`);
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "r",
    code: "KeyR",
    modifiers: 2,
    windowsVirtualKeyCode: 82,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "r",
    code: "KeyR",
    modifiers: 2,
    windowsVirtualKeyCode: 82,
  });
  const editorFirstText = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]').snapshot().text`);
  if (!editorFirstText.startsWith("- ")) {
    throw new Error(`editor-first Ctrl+R was not intercepted: ${editorFirstText}`);
  }
  await assertTaskShortcutCarets(page, evaluate);

  const textBeforeTab = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]').snapshot().text`);
  await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab" });
  await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab" });
  const tabState = await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    return {
      text: host.snapshot().text,
      ownsFocus: document.activeElement === host
        && host.shadowRoot.activeElement === host.shadowRoot.querySelector('textarea'),
    };
  })()`);
  if (!tabState.text.startsWith("\t") || !tabState.ownsFocus) {
    throw new Error(`Tab did not indent while retaining editor focus: ${JSON.stringify(tabState)}`);
  }

  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "Tab",
    code: "Tab",
    modifiers: 8,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "Tab",
    code: "Tab",
    modifiers: 8,
  });
  const textAfterShiftTab = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]').snapshot().text`);
  if (textAfterShiftTab !== textBeforeTab) {
    throw new Error("Shift+Tab did not restore the outdented source text");
  }

  await assertPhysicalEnter(page, "    alpha", "    alpha\n    ", 0, "preserve indentation");
  await assertPhysicalEnter(page, "- item", "- item\n- ", 0, "continue a bullet");
  await assertPhysicalEnter(page, "- [x] done", "- [x] done\n- [ ] ", 0, "continue a task unchecked");
  await assertPhysicalEnter(page, "    alpha", "    alpha\n", 8, "insert a raw Shift+Enter newline");

  await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape" });
  await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape" });
  await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab" });
  await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab" });
  const nextFocus = await evaluate(page, "document.activeElement?.id");
  if (nextFocus !== "pointer-next") {
    throw new Error(`Escape then Tab did not leave the editor for the next control: ${nextFocus}`);
  }

  await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]').focus()`);
  await page.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape" });
  await page.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape" });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "Tab",
    code: "Tab",
    modifiers: 8,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "Tab",
    code: "Tab",
    modifiers: 8,
  });
  const previousFocus = await evaluate(page, "document.activeElement?.id");
  if (previousFocus !== "pointer-previous") {
    throw new Error(`Escape then Shift+Tab did not reach the previous control: ${previousFocus}`);
  }
  return {
    caret,
    visualCaret,
    arrowCaret,
    multiClickSelections,
    dragSelectionLength: wrappedSelection.length,
    wrappedSelectionVisualRows: wrappedSelection.visualRows,
    editorFirstReloadIntercepted: true,
    projectedWrappedPointerCaret: caret,
    taskCaretPreserved: true,
    physicalEnterSmartNewline: true,
    physicalShiftEnterRawNewline: true,
    realModifierClickCarets: pointerCarets.selections.length,
    realVerticalCarets: verticalCarets.selections.length,
    tabIndented: true,
    shiftTabOutdented: true,
    wordSelectionLength,
    escapeTabTarget: nextFocus,
    escapeShiftTabTarget: previousFocus,
  };
}

async function assertPhysicalEnter(page, source, expected, modifiers, behavior) {
  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Pointer hit-test document"]');
    const input = host.shadowRoot.querySelector('textarea');
    host.value = ${JSON.stringify(source)};
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
    input.dispatchEvent(new Event('select', { bubbles: true }));
  })()`);
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "Enter",
    code: "Enter",
    modifiers,
    windowsVirtualKeyCode: 13,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "Enter",
    code: "Enter",
    modifiers,
    windowsVirtualKeyCode: 13,
  });
  const actual = await evaluate(page, `document.querySelector('[aria-label="Pointer hit-test document"]').snapshot().text`);
  if (actual !== expected) {
    throw new Error(`physical Enter did not ${behavior}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

async function auditCaretFollow(page) {
  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Caret follow document"]');
    host.scrollIntoView({ block: 'center' });
    host.focus();
    const input = host.shadowRoot.querySelector('textarea');
    input.setSelectionRange(input.value.length, input.value.length);
    window.__caretFollowSelectionFrame = new Promise((resolve) => {
      host.addEventListener('continuity-frame', resolve, { once: true });
    });
    input.dispatchEvent(new Event('select', { bubbles: true }));
  })()`);
  await page.send("Runtime.evaluate", {
    expression: "window.__caretFollowSelectionFrame", awaitPromise: true, returnByValue: true,
  });
  await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Caret follow document"]');
    const input = host.shadowRoot.querySelector('textarea');
    input.scrollTop = 0;
    input.dispatchEvent(new Event('scroll'));
    window.__caretFollowFrame = new Promise((resolve) => {
      host.addEventListener('continuity-frame', resolve, { once: true });
    });
  })()`);
  await page.send("Input.insertText", { text: "real-browser-input" });
  await page.send("Runtime.evaluate", {
    expression: "window.__caretFollowFrame", awaitPromise: true, returnByValue: true,
  });
  const state = await evaluate(page, `(() => {
    const host = document.querySelector('[aria-label="Caret follow document"]');
    const input = host.shadowRoot.querySelector('textarea');
    return {
      scrollTop: input.scrollTop,
      selectionAtEnd: input.selectionEnd === input.value.length,
      projectionTransform: host.shadowRoot.querySelector('.projection').style.transform,
    };
  })()`);
  const expectedTransform = `translate(0px, ${-state.scrollTop}px)`;
  if (state.scrollTop <= 0 || !state.selectionAtEnd || state.projectionTransform !== expectedTransform) {
    throw new Error(`real browser typing did not follow the caret: ${JSON.stringify(state)}`);
  }
  return state;
}

function devtoolsEndpoint(child) {
  return new Promise((resolveEndpoint, reject) => {
    let stderr = "";
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
      const match = /DevTools listening on (ws:\/\/[^\s]+)/u.exec(stderr);
      if (match) {
        resolveEndpoint(match[1]);
      }
    });
    child.once("error", reject);
    child.once("exit", (code) => reject(new Error(`Chromium exited ${code}: ${stderr}`)));
  });
}

async function waitForPageTarget(port) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json());
      const target = targets.find((candidate) => candidate.type === "page");
      if (target) {
        return target;
      }
    } catch {
      // The endpoint may accept WebSockets before its HTTP target list is ready.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error("Chromium page target did not become ready");
}

function discoverChromium() {
  const candidates = [
    process.env.CONTINUITY_CHROMIUM,
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean);
  const path = candidates.find((candidate) => existsSync(candidate));
  if (!path) {
    throw new Error("Chromium not found; set CONTINUITY_CHROMIUM");
  }
  return path;
}

await runBrowserContract();
