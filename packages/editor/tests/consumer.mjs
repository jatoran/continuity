import assert from "node:assert/strict";
import { readdir, readFile, stat } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { performance } from "node:perf_hooks";

import { Editor, initialize } from "@continuity-editor/editor";

const fixture = JSON.parse(await readFile(process.argv[2], "utf8"));
const wasmUrl = new URL(import.meta.resolve("@continuity-editor/editor/wasm"));
const wasmBytes = await readFile(wasmUrl);

const initializationStarted = performance.now();
await initialize({ wasm: wasmBytes });
const initializationMs = performance.now() - initializationStarted;

assertMultiCursor(fixture.multiCursor);
assertDeleteBackward(fixture.deleteBackward);
assertNoOp(fixture.noOp);
assertUndo(fixture.undo);
assertUndoBranch(fixture.undoBranch);
assertProjection(fixture.projection);
assertRestoredRevision();
assertPortableCommands();
const metrics = await measureBudgets(initializationMs, wasmBytes);

console.log(`CONTINUITY_WASM_METRICS ${JSON.stringify(metrics)}`);

function assertMultiCursor(testCase) {
  const editor = new Editor(testCase.initialText);
  editor.setSelections(testCase.selections);
  const change = editor.insertText(testCase.insertText, 1_000);
  assert.deepEqual(change.deltas, testCase.expectedDeltas);
  const snapshot = editor.snapshot();
  assert.equal(snapshot.text, testCase.expectedText);
  assert.equal(snapshot.revision, testCase.expectedRevision);
  assert.deepEqual(
    snapshot.selections.map(({ head }) => [head.line, head.byteInLine]),
    testCase.expectedCarets,
  );
  editor.destroy();
}

function assertUndo(testCase) {
  const editor = new Editor();
  testCase.typing.forEach((text, index) => {
    editor.insertText(text, 2_000 + index * 100);
  });
  assert.equal(editor.snapshot().text, testCase.expectedText);
  editor.undo(2_400);
  assert.equal(editor.snapshot().text, testCase.expectedAfterUndo);
  editor.redo(2_500);
  assert.equal(editor.snapshot().text, testCase.expectedAfterRedo);
  editor.destroy();
}

function assertDeleteBackward(testCase) {
  const editor = new Editor(testCase.initialText);
  editor.setSelections([testCase.selection]);
  editor.deleteBackward(1_500);
  const snapshot = editor.snapshot();
  assert.equal(snapshot.text, testCase.expectedText);
  assert.deepEqual(
    [snapshot.selections[0].head.line, snapshot.selections[0].head.byteInLine],
    testCase.expectedCaret,
  );
  editor.destroy();
}

function assertNoOp(testCase) {
  const editor = new Editor(testCase.initialText);
  const change = editor.insertText("", 1_600);
  assert.equal(change.revisionAfter, testCase.expectedRevision);
  assert.deepEqual(change.deltas, testCase.expectedDeltas);
  const snapshot = editor.snapshot();
  assert.equal(snapshot.text, testCase.initialText);
  assert.equal(snapshot.revision, testCase.expectedRevision);
  editor.destroy();
}

function assertUndoBranch(testCase) {
  const editor = new Editor();
  editor.insertText(testCase.inputs[0], 3_000);
  editor.insertText(testCase.inputs[1], 3_001);
  editor.undo(3_002);
  editor.insertText(testCase.inputs[2], 3_003);
  assert.equal(editor.snapshot().text, testCase.expectedReplacement);
  editor.undo(3_004);
  editor.redoAlternate(3_005);
  assert.equal(editor.snapshot().text, testCase.expectedAlternate);
  editor.destroy();
}

function assertProjection(testCase) {
  const editor = new Editor(testCase.source);
  assert.deepEqual(editor.projection(), testCase.expected);
  editor.destroy();
}

function assertRestoredRevision() {
  const editor = new Editor("persisted", 41);
  assert.equal(editor.snapshot().revision, 41);
  editor.setSelections([{
    anchor: { line: 0, byteInLine: 9 },
    head: { line: 0, byteInLine: 9 },
    kind: "caret",
  }]);
  editor.insertText("!", 3_500);
  assert.equal(editor.snapshot().revision, 42);
  editor.destroy();
  assert.throws(() => new Editor("invalid", -1), RangeError);
}

function assertPortableCommands() {
  const editor = new Editor("task");
  editor.executeCommand("markdown.toggle_task", 3_600);
  assert.equal(editor.snapshot().text, "- [ ] task");
  editor.executeCommand("editor.undo", 3_601);
  assert.equal(editor.snapshot().text, "task");
  assert.throws(() => editor.executeCommand("file.open"), /unsupported editor command/u);
  editor.destroy();
}

async function measureBudgets(initializationMs, wasmBytes) {
  const editor = new Editor();
  for (let index = 0; index < 50; index += 1) {
    editor.insertText("x", 3_000 + index);
  }
  const memoryBefore = editor.linearMemoryBytes();
  const samples = [];
  for (let index = 0; index < 1_000; index += 1) {
    const started = performance.now();
    editor.insertText("x", 4_000 + index);
    samples.push(performance.now() - started);
  }
  const memoryAfter = editor.linearMemoryBytes();
  editor.destroy();

  samples.sort((left, right) => left - right);
  const editP99Ms = samples[Math.ceil(samples.length * 0.99) - 1];
  const memoryGrowthBytes = memoryAfter - memoryBefore;
  const gzipBytes = gzipSync(wasmBytes, { level: 9 }).byteLength;
  const rangePresentationP99Ms = measureRangePresentation();
  const packageSizes = await measurePackageSizes(new URL("../", wasmUrl));
  const metrics = {
    initializationMs,
    editP99Ms,
    memoryGrowthBytes,
    wasmBytes: wasmBytes.byteLength,
    gzipBytes,
    rangePresentationP99Ms,
    ...packageSizes,
  };

  assert.ok(initializationMs <= 100, `WASM initialization ${initializationMs} ms > 100 ms`);
  assert.ok(editP99Ms <= 4, `WASM edit p99 ${editP99Ms} ms > 4 ms`);
  assert.ok(memoryGrowthBytes <= 16 * 1024 * 1024, `WASM memory growth ${memoryGrowthBytes} > 16 MiB`);
  assert.ok(gzipBytes <= 700 * 1024, `WASM gzip size ${gzipBytes} > 700 KiB`);
  assert.ok(rangePresentationP99Ms <= 100, `edited viewport presentation p99 ${rangePresentationP99Ms} ms > 100 ms`);
  // Raised 180 -> 208 KiB on 2026-07-22 for the command rail, its settings
  // panel, and the line-scoped composition preview (unminified source ships).
  // Raised 208 -> 264 KiB on 2026-07-24 for the touch input surface: the shield
  // that keeps the finger off the textarea, its scroll ownership, the
  // projection-owned long-press gesture, drag auto-scroll, the selection action
  // bar that replaces the platform bubble, and the clipboard bridge — plus the
  // module splits the 600-line file cap forces those to be delivered in.
  // Raised 264 -> 288 KiB on 2026-07-25 for the command-rail extension surface:
  // the host action registry, arrangement reconciliation that retains ids for
  // actions registered after load, and visible-row caret motion.
  // Raised 288 -> 320 KiB on 2026-07-30 for projection chrome: host range
  // decorations, opt-in indent guides, touch selection adjust handles, the
  // shared overlay geometry the three of them and the selection now measure
  // through, and serializable undo history.
  // Raised 320 -> 336 KiB on 2026-08-06 for the soft-keyboard gate: the Android
  // keyboard is not a function of DOM focus, so refusing to focus on the touch
  // path is not enough to keep a selection gesture from raising it, and the
  // `inputmode` state machine that is enough has to be documented where it is
  // read. 0.2.20 shipped 267 bytes under the old ceiling, so this restores a
  // working margin rather than only clearing the change.
  assert.ok(packageSizes.javascriptBytes <= 336 * 1024, `package JavaScript ${packageSizes.javascriptBytes} > 336 KiB`);
  assert.ok(packageSizes.lazyEntryBytes <= 2 * 1024, `lazy entry ${packageSizes.lazyEntryBytes} > 2 KiB`);
  assert.ok(packageSizes.installedBytes <= 2 * 1024 * 1024, `installed package ${packageSizes.installedBytes} > 2 MiB`);
  return metrics;
}

function measureRangePresentation() {
  const lines = Array.from({ length: 10_000 }, (_, index) => `- line ${index} **markdown**`).join("\n");
  const editor = new Editor(lines);
  editor.setSelections([{
    anchor: { line: 9_999, byteInLine: 24 },
    head: { line: 9_999, byteInLine: 24 },
    kind: "caret",
  }]);
  editor.presentationRange(9_920, 10_000);
  const samples = [];
  for (let index = 0; index < 32; index += 1) {
    editor.insertText("x", 10_000 + index);
    const started = performance.now();
    editor.presentationRange(9_920, 10_000);
    samples.push(performance.now() - started);
  }
  editor.destroy();
  samples.sort((left, right) => left - right);
  return samples[Math.ceil(samples.length * 0.99) - 1];
}

async function measurePackageSizes(root) {
  let installedBytes = 0;
  let javascriptBytes = 0;
  async function walk(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const url = new URL(`${entry.name}${entry.isDirectory() ? "/" : ""}`, directory);
      if (entry.isDirectory()) await walk(url);
      else {
        const bytes = (await stat(url)).size;
        installedBytes += bytes;
        if (entry.name.endsWith(".js")) javascriptBytes += bytes;
      }
    }
  }
  await walk(root);
  return {
    installedBytes,
    javascriptBytes,
    lazyEntryBytes: (await stat(new URL("lazy.js", root))).size,
  };
}
