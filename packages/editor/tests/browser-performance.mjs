import {
  ContinuityEditorElement,
  Editor,
} from "/node_modules/@continuity-editor/editor/index.js";

/** Measure long-document engine, input-frame, newline, and wrap costs. */
export async function runPerformanceTests(check) {
  const gate = await measureGateDocument();
  const bulkPaste = await measureBulkPaste();
  const longDocument = await measureLongDocument();
  check(gate.readyMs <= 2_000, `large-document ready ${gate.readyMs} ms exceeds 2,000 ms`);
  check(gate.scrollMs <= 100, `large-document scroll ${gate.scrollMs} ms exceeds 100 ms`);
  check(gate.inputFrameP99Ms <= 50, `input-to-paint-ready p99 ${gate.inputFrameP99Ms} ms exceeds 50 ms`);
  check(gate.inputFrameP999Ms <= 80, `input-to-paint-ready p99.9 ${gate.inputFrameP999Ms} ms exceeds 80 ms`);
  check(longDocument.readyMs <= 2_500, `10k-line ready ${longDocument.readyMs} ms exceeds 2,500 ms`);
  check(longDocument.typing.dispatchP99Ms <= 160, `10k-line typing dispatch p99 ${longDocument.typing.dispatchP99Ms} ms exceeds 160 ms`);
  check(longDocument.typing.frameP99Ms <= 300, `10k-line typing frame p99 ${longDocument.typing.frameP99Ms} ms exceeds 300 ms`);
  check(longDocument.newline.frameP99Ms <= 250, `10k-line newline frame p99 ${longDocument.newline.frameP99Ms} ms exceeds 250 ms`);
  check(longDocument.wrap.p99Ms <= 200, `10k-line wrap p99 ${longDocument.wrap.p99Ms} ms exceeds 200 ms`);
  check(longDocument.headless.presentationP50Ms <= 100, `warm compact presentation p50 ${longDocument.headless.presentationP50Ms} ms exceeds 100 ms`);
  check(longDocument.headless.rangePresentationP99Ms <= 100, `edited viewport presentation p99 ${longDocument.headless.rangePresentationP99Ms} ms exceeds 100 ms`);
  check(longDocument.viewportDetailed, "scrolled projection viewport did not realize inline DOM");
  check(bulkPaste.dispatchMs <= 250, `600-line paste dispatch ${bulkPaste.dispatchMs} ms exceeds 250 ms`);
  check(bulkPaste.frameMs <= 400, `600-line paste frame ${bulkPaste.frameMs} ms exceeds 400 ms`);
  check(bulkPaste.hasViewportCoverage, "600-line paste left the visible viewport without projection geometry");
  return {
    lineCount: gate.lineCount,
    sampleCount: gate.sampleCount,
    largeDocumentReadyMs: gate.readyMs,
    largeDocumentScrollMs: gate.scrollMs,
    inputFrameP99Ms: gate.inputFrameP99Ms,
    inputFrameP999Ms: gate.inputFrameP999Ms,
    memoryBytes: gate.memoryBytes,
    longDocument,
    bulkPaste,
  };
}

async function measureGateDocument() {
  const lineCount = 1_500;
  const editor = createComponent(lineCount, "Performance document");
  const readyStarted = performance.now();
  await editor.ready;
  const readyMs = performance.now() - readyStarted;
  const input = editor.shadowRoot.querySelector("textarea");
  input.setSelectionRange(input.value.length, input.value.length);
  const samples = [];
  for (let index = 0; index < 1_024; index += 1) {
    const frame = once(editor, "continuity-frame");
    dispatchBeforeInput(input, "insertText", "x");
    samples.push((await frame).detail.latencyMs);
  }
  const scrollStarted = performance.now();
  input.scrollTop = input.scrollHeight;
  input.dispatchEvent(new Event("scroll"));
  await animationFrames(2);
  const scrollMs = performance.now() - scrollStarted;
  samples.sort((left, right) => left - right);
  const result = {
    lineCount,
    sampleCount: samples.length,
    readyMs,
    scrollMs,
    inputFrameP99Ms: percentile(samples, 0.99),
    inputFrameP999Ms: percentile(samples, 0.999),
    memoryBytes: performance.memory?.usedJSHeapSize ?? null,
  };
  editor.destroy();
  editor.remove();
  return result;
}

async function measureLongDocument() {
  const lineCount = 10_000;
  const source = createSource(lineCount);
  const headless = measureHeadless(source, lineCount);
  const editor = createComponent(lineCount, "Long performance document");
  editor.style.cssText = "display:block;width:760px;height:420px";
  const readyStarted = performance.now();
  await editor.ready;
  const readyMs = performance.now() - readyStarted;
  const input = editor.shadowRoot.querySelector("textarea");
  input.setSelectionRange(input.value.length, input.value.length);
  input.scrollTop = input.scrollHeight;
  input.dispatchEvent(new Event("scroll"));
  const viewportDetailed = editor.shadowRoot.querySelector(".projection")
    .lastElementChild?.dataset.detailed === "true";
  const typing = await measureInput(editor, input, "insertText", "x", 64);
  const newline = await measureInput(editor, input, "insertLineBreak", null, 16);
  const wrap = await measureWrap(editor, input, 8);
  const result = {
    lineCount,
    sourceBytes: new TextEncoder().encode(source).byteLength,
    readyMs,
    headless,
    typing,
    newline,
    wrap,
    viewportDetailed,
    domLines: editor.shadowRoot.querySelectorAll(".projection > .line").length,
    memoryBytes: performance.memory?.usedJSHeapSize ?? null,
  };
  editor.destroy();
  editor.remove();
  return result;
}

function measureHeadless(source, lineCount) {
  const editor = new Editor(source);
  editor.setSelections([caret(lineCount - 1, source.split("\n").at(-1).length)]);
  const editSamples = [];
  for (let index = 0; index < 64; index += 1) {
    const startedAt = performance.now();
    editor.insertText("x");
    editSamples.push(performance.now() - startedAt);
  }
  const projectionSamples = [];
  const presentationSamples = [];
  for (let index = 0; index < 5; index += 1) {
    const startedAt = performance.now();
    editor.presentation();
    presentationSamples.push(performance.now() - startedAt);
  }
  const rangePresentationSamples = [];
  for (let index = 0; index < 32; index += 1) {
    editor.insertText("x");
    const startedAt = performance.now();
    editor.presentationRange(lineCount - 80, lineCount);
    rangePresentationSamples.push(performance.now() - startedAt);
  }
  for (let index = 0; index < 5; index += 1) {
    const startedAt = performance.now();
    editor.projection();
    projectionSamples.push(performance.now() - startedAt);
  }
  editSamples.sort((left, right) => left - right);
  projectionSamples.sort((left, right) => left - right);
  presentationSamples.sort((left, right) => left - right);
  rangePresentationSamples.sort((left, right) => left - right);
  const result = {
    editP50Ms: percentile(editSamples, 0.5),
    editP99Ms: percentile(editSamples, 0.99),
    projectionP50Ms: percentile(projectionSamples, 0.5),
    projectionP99Ms: percentile(projectionSamples, 0.99),
    presentationP50Ms: percentile(presentationSamples, 0.5),
    presentationP99Ms: percentile(presentationSamples, 0.99),
    rangePresentationP50Ms: percentile(rangePresentationSamples, 0.5),
    rangePresentationP99Ms: percentile(rangePresentationSamples, 0.99),
  };
  editor.destroy();
  return result;
}

async function measureBulkPaste() {
  const editor = new ContinuityEditorElement();
  editor.spellcheck = false;
  editor.setAttribute("aria-label", "Bulk paste performance document");
  editor.style.cssText = "display:block;width:520px;height:300px";
  document.body.append(editor);
  await editor.ready;
  const input = editor.shadowRoot.querySelector("textarea");
  const source = createSource(600);
  const frame = once(editor, "continuity-frame");
  const startedAt = performance.now();
  dispatchBeforeInput(input, "insertText", source);
  const dispatchMs = performance.now() - startedAt;
  const frameMs = (await frame).detail.latencyMs;
  await animationFrames(2);
  const projection = editor.shadowRoot.querySelector(".projection");
  const hasViewportCoverage = projection.scrollHeight >= input.scrollTop + input.clientHeight;
  editor.destroy();
  editor.remove();
  return { dispatchMs, frameMs, hasViewportCoverage };
}

async function measureInput(editor, input, inputType, data, count) {
  const dispatchSamples = [];
  const frameSamples = [];
  for (let index = 0; index < count; index += 1) {
    const frame = once(editor, "continuity-frame");
    const startedAt = performance.now();
    dispatchBeforeInput(input, inputType, data);
    dispatchSamples.push(performance.now() - startedAt);
    frameSamples.push((await frame).detail.latencyMs);
  }
  dispatchSamples.sort((left, right) => left - right);
  frameSamples.sort((left, right) => left - right);
  return sampleReport(dispatchSamples, frameSamples);
}

async function measureWrap(editor, input, count) {
  const samples = [];
  for (let index = 0; index < count; index += 1) {
    const frame = once(editor, "continuity-frame");
    const startedAt = performance.now();
    editor.style.width = index % 2 === 0 ? "380px" : "760px";
    await frame;
    samples.push(performance.now() - startedAt);
  }
  samples.sort((left, right) => left - right);
  return { p50Ms: percentile(samples, 0.5), p99Ms: percentile(samples, 0.99) };
}

function sampleReport(dispatchSamples, frameSamples) {
  return {
    dispatchP50Ms: percentile(dispatchSamples, 0.5),
    dispatchP99Ms: percentile(dispatchSamples, 0.99),
    frameP50Ms: percentile(frameSamples, 0.5),
    frameP99Ms: percentile(frameSamples, 0.99),
  };
}

function createComponent(lineCount, label) {
  const editor = new ContinuityEditorElement();
  editor.spellcheck = false;
  editor.value = createSource(lineCount);
  editor.setAttribute("aria-label", label);
  document.body.append(editor);
  return editor;
}

function createSource(lineCount) {
  return Array.from(
    { length: lineCount },
    (_, index) => `- [ ] line ${index}: **markdown** text that wraps across narrower embedded hosts`,
  ).join("\n");
}

function dispatchBeforeInput(input, inputType, data) {
  input.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    data,
    inputType,
  }));
}

function caret(line, byteInLine) {
  const position = { line, byteInLine };
  return { anchor: position, head: position, kind: "caret" };
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

function percentile(sorted, fraction) {
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)];
}
