import assert from "node:assert/strict";
import test from "node:test";

const moduleUrl = new URL(
  "./node_modules/@continuity-editor/editor/controller.js",
  import.meta.url,
);
const {
  attachContinuityEditor,
} = await import(moduleUrl);

test("initialization seeds text and revision before readiness", () => {
  const editor = new FakeEditor();
  const controller = attachContinuityEditor(editor, { value: "# Note", revision: 12 });
  assert.equal(editor.value, "# Note");
  assert.equal(editor.initialRevision, 12);
  controller.dispose();
});

test("matching controlled snapshot does not echo a replacement", async () => {
  const editor = new FakeEditor("same", 4);
  const controller = attachContinuityEditor(editor, { value: "same", revision: 4 });
  assert.equal(await controller.synchronize("same", 4), null);
  assert.deepEqual(editor.replacements, []);
});

test("host replacement uses the supplied Continuity revision", async () => {
  const editor = new FakeEditor("old", 7);
  const controller = attachContinuityEditor(editor, { value: "old", revision: 7 });
  const change = await controller.synchronize("new", 7);
  assert.deepEqual(change, { value: "new", expectedRevision: 7 });
  assert.deepEqual(editor.replacements, [{ value: "new", expectedRevision: 7 }]);
});

test("stale host replacement propagates its revision conflict", async () => {
  const editor = new FakeEditor("newer typing", 9);
  const controller = attachContinuityEditor(editor, { value: "original", revision: 8 });
  await assert.rejects(
    controller.synchronize("stale server value", 8),
    /expected 8, actual 9/u,
  );
});

test("controlled reconciliation is suppressed while the element is composing", async () => {
  const editor = new FakeEditor("word", 5);
  editor.composing = true;
  const controller = attachContinuityEditor(editor, { value: "word", revision: 5 });
  assert.equal(await controller.synchronize("word STALE", 5), null);
  assert.deepEqual(editor.replacements, []);
  editor.composing = false;
  assert.deepEqual(await controller.synchronize("word committed", 5), {
    value: "word committed", expectedRevision: 5,
  });
});

test("configuration updates properties and optional attributes", () => {
  const editor = new FakeEditor();
  const controller = attachContinuityEditor(editor, { value: "", revision: 0 });
  controller.configure({
    readOnly: true,
    spellcheck: false,
    shortcutPolicy: "editor-first",
    shortcutBindings: { "Mod+R": "editor.redo" },
    theme: "dark",
    tabBehavior: "focus",
    syntax: "plain",
  });
  assert.equal(editor.readOnly, true);
  assert.equal(editor.spellcheck, false);
  assert.equal(editor.shortcutPolicy, "editor-first");
  assert.deepEqual(editor.shortcutBindings, { "Mod+R": "editor.redo" });
  assert.equal(editor.attributes.get("theme"), "dark");
  assert.equal(editor.attributes.get("tab-behavior"), "focus");
  assert.equal(editor.attributes.get("syntax"), "plain");
  controller.configure({ theme: "light" });
  assert.equal(editor.readOnly, true);
  assert.equal(editor.attributes.get("theme"), "light");
});

test("controller exposes replacement, selection, viewport, and memory contracts", async () => {
  const editor = new FakeEditor("old", 2);
  const controller = attachContinuityEditor(editor, { value: "old", revision: 2 });
  assert.deepEqual(await controller.replaceCurrent("new"), { value: "new", expectedRevision: 2 });
  const selections = [{
    anchor: { line: 0, byteInLine: 1 }, head: { line: 0, byteInLine: 1 }, kind: "caret",
  }];
  controller.setSelections(selections, { reveal: false });
  controller.revealRange({ start: selections[0].head, end: selections[0].head });
  controller.restoreScrollState({ top: 10, left: 2 });
  assert.deepEqual(controller.getScrollState(), { top: 10, left: 2 });
  assert.equal(controller.linearMemoryBytes(), 65_536);
});

test("host replacement has a dedicated callback without requiring onChange", () => {
  const editor = new FakeEditor();
  const details = [];
  const controller = attachContinuityEditor(editor, {
    value: "", revision: 0, callbacks: { onHostReplacement: (detail) => details.push(detail) },
  });
  editor.emit("continuity-change", { commitOrigin: "host", sequence: 1 });
  assert.equal(details.length, 1);
  controller.dispose();
});

test("attachment rejects an omitted controlled value", () => {
  assert.throws(
    () => attachContinuityEditor(new FakeEditor(), { revision: 0 }),
    /controlled value is required/u,
  );
});

test("callbacks update without listener churn and dispose detaches them", () => {
  const editor = new FakeEditor();
  const details = [];
  const controller = attachContinuityEditor(editor, {
    value: "",
    revision: 0,
    callbacks: { onChange: (detail) => details.push(`first:${detail.sequence}`) },
  });
  editor.emit("continuity-change", { sequence: 1 });
  controller.setCallbacks({ onChange: (detail) => details.push(`second:${detail.sequence}`) });
  editor.emit("continuity-change", { sequence: 2 });
  controller.dispose();
  editor.emit("continuity-change", { sequence: 3 });
  assert.deepEqual(details, ["first:1", "second:2"]);
  assert.throws(() => controller.configure({ readOnly: true }), /disposed/u);
});

class FakeEditor {
  constructor(text = "", revision = 0) {
    this.current = { text, revision };
    this.ready = Promise.resolve(this);
    this.replacements = [];
    this.attributes = new Map();
    this.listeners = new Map();
    this.scrollState = { top: 0, left: 0 };
    this.composing = false;
  }

  snapshot() {
    return this.current;
  }

  replaceValue(value, expectedRevision) {
    if (expectedRevision !== this.current.revision) {
      throw new Error(`revision mismatch: expected ${expectedRevision}, actual ${this.current.revision}`);
    }
    const change = { value, expectedRevision };
    this.replacements.push(change);
    this.current = { text: value, revision: expectedRevision + 1 };
    return change;
  }

  setSelections(selections, options) { this.selections = { selections, options }; }
  revealRange(range, options) { this.revealed = { range, options }; }
  getScrollState() { return this.scrollState; }
  restoreScrollState(state) { this.scrollState = state; }
  linearMemoryBytes() { return 65_536; }

  setAttribute(name, value) {
    this.attributes.set(name, value);
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  addEventListener(name, listener) {
    const listeners = this.listeners.get(name) ?? new Set();
    listeners.add(listener);
    this.listeners.set(name, listeners);
  }

  removeEventListener(name, listener) {
    this.listeners.get(name)?.delete(listener);
  }

  emit(name, detail) {
    for (const listener of this.listeners.get(name) ?? []) {
      listener({ detail });
    }
  }
}
