const params = new URLSearchParams(location.search);
const build = params.get("build") === "baseline" ? "baseline" : "fixed";
document.querySelector("#build").textContent = build;
document.querySelector("#build").dataset.build = build;

// This module was itself loaded as `app.mjs?v=<build>`. Carry that id onto every
// dynamic import so the editor source can never be served from a phone's module
// cache while the page believes it is fresh.
const buildId = new URL(import.meta.url).searchParams.get("v") ?? "dev";
const bust = (path) => `${path}?v=${buildId}`;

const { ContinuityEditorElement } = await import(bust(`./${build}/index.js`));
// Reach into the built source for the exact hit-test the editor uses, so the
// trace can report what a screen point resolves to rather than inferring it.
const { projectionLineLayout, projectionPositionAtPoint } = await import(bust(`./${build}/src/projection.js`));
const { caretDisplayOffset, measureTextOffset } = await import(bust(`./${build}/src/visual_carets.js`));

const LONG = "The quick brown fox jumps over the lazy dog while the writer keeps typing";

const CORPORA = {
  mixed: () => Array.from({ length: 12 }, (_, i) => [
    `## Section ${i}: ${LONG}`,
    `${LONG} body paragraph ${i} with **bold text**, \`inline code\`, and [a link](https://example.com/target/${i}).`,
    `- list item ${i} that is long enough to wrap beneath the bullet hanging indent`,
    `> quoted line ${i}: ${LONG}`,
    "",
  ].join("\n")).join("\n"),

  // Reproduces the unreachable-tail bug: projected headings render at 1.45em in
  // a narrower box, so they wrap into more rows than the raw textarea does.
  longHeadings: () => Array.from({ length: 40 }, (_, i) => `# ${LONG} ${i}`).join("\n"),

  // Every marker folds away, so projected text is much shorter than source.
  folded: () => Array.from({ length: 30 }, (_, i) => (
    `line ${i} with **bold text ${i}** and \`inline code ${i}\` and [link ${i}](https://example.com/a/very/long/target/${i}) trailing words`
  )).join("\n"),

  lists: () => Array.from({ length: 30 }, (_, i) => `- item ${i}: ${LONG} ${LONG}`).join("\n"),

  bigPaste: () => Array.from({ length: 200 }, (_, i) => [
    `# Chapter ${i}: ${LONG}`,
    `${LONG} ${LONG}`,
    `- point ${i} with **emphasis** and \`code\``,
  ].join("\n")).join("\n"),

  empty: () => "",
};

const mount = document.querySelector("#mount");
const editor = new ContinuityEditorElement();
editor.setAttribute("aria-label", "Mobile playground document");
editor.setAttribute("theme", "dark");
editor.setAttribute("command-rail", "auto");
editor.value = CORPORA.mixed();
mount.append(editor);
await editor.ready;

const shadow = editor.shadowRoot;
const input = shadow.querySelector("textarea");
const projection = shadow.querySelector(".projection");
const frame = shadow.querySelector(".frame");
const shield = shadow.querySelector(".touch-shield");
// Whichever element owns scrolling: the shield on touch, the textarea on a
// mouse. Reading the wrong one reports a scroll extent nothing is using.
const scroller = () => (
  frame.classList.contains("touch-scrolling") && shield ? shield : input
);
const readout = document.querySelector("#readout");

document.querySelector("#corpus").addEventListener("change", (event) => {
  editor.value = CORPORA[event.target.value]();
  input.scrollTop = 0;
  input.dispatchEvent(new Event("scroll"));
});

document.querySelector("#theme").addEventListener("click", () => {
  editor.setAttribute("theme", editor.getAttribute("theme") === "dark" ? "light" : "dark");
});

document.querySelector("#bottom").addEventListener("click", async () => {
  const view = scroller();
  // Realizing projected detail near the floor changes line heights and so the
  // extent; re-apply until it stops moving instead of assuming one jump lands.
  let previous = -1;
  for (let attempt = 0; attempt < 12 && view.scrollTop !== previous; attempt += 1) {
    previous = view.scrollTop;
    view.scrollTop = view.scrollHeight;
    view.dispatchEvent(new Event("scroll"));
    await new Promise((resolve) => requestAnimationFrame(resolve));
  }
  render();
});

/**
 * Keep the page sized to the visual viewport. The soft keyboard shrinks the
 * visual viewport but not the layout viewport, so a 100%-height page keeps its
 * bottom rows underneath the keyboard — which reads as "the end is unreachable"
 * regardless of what the editor's own scroll extent is doing.
 */
function applyViewportHeight() {
  const height = visualViewport?.height ?? innerHeight;
  document.documentElement.style.setProperty("--app-height", `${Math.round(height)}px`);
}
applyViewportHeight();
visualViewport?.addEventListener("resize", applyViewportHeight);
visualViewport?.addEventListener("scroll", applyViewportHeight);
addEventListener("resize", applyViewportHeight);

document.querySelector("#swap").addEventListener("click", () => {
  params.set("build", build === "fixed" ? "baseline" : "fixed");
  location.search = params.toString();
});

let showDiagnostics = true;
document.querySelector("#diag").addEventListener("click", (event) => {
  showDiagnostics = !showDiagnostics;
  event.target.classList.toggle("on", showDiagnostics);
  readout.hidden = !showDiagnostics;
});

// A rolling trace of what the platform did versus what the engine adopted.
// Touch bugs here are timing-shaped, so a phone has to show the sequence and
// the outcome of each event, not just the end state.
const TRACE_LIMIT = 400;
const trace = [];
let traceEnabled = false;
let traceStart = 0;

/**
 * Everything needed to tell a hit-test fault from a drawing fault at one point:
 * what the editor's own hit-test resolves the point to, which line element sits
 * under it, and where the engine's current caret is actually painted.
 */
function probePoint(clientX, clientY) {
  const hit = projectionPositionAtPoint(projection, clientX, clientY);
  const children = [...projection.children];
  // The element the finger is physically over, independent of the hit-test.
  const overIndex = children.findIndex((child) => {
    const rect = child.getBoundingClientRect();
    return clientY >= rect.top && clientY < rect.bottom;
  });
  const over = children[overIndex];
  const overRect = over?.getBoundingClientRect();
  const caretRect = editor.shadowRoot.querySelector(".visual-caret")?.getBoundingClientRect();
  // Round-trip: where does the position the hit-test returned actually render?
  // A vertical error near one row height is the "highlight starts a line below"
  // fault; zero error there means the fault is in painting, not in mapping.
  let roundTrip = "rt=?";
  if (hit) {
    const layout = projectionLineLayout(projection, hit.line);
    if (layout) {
      const point = measureTextOffset(layout.element, caretDisplayOffset(layout, hit.byteInLine));
      roundTrip = `rt=${Math.round(point.left)},${Math.round(point.top)}`
        + ` dy=${Math.round(point.top + point.height / 2 - clientY)}`
        + ` dx=${Math.round(point.left - clientX)}`;
    }
  }
  const highlight = editor.shadowRoot.querySelector(".visual-selection")?.getBoundingClientRect();
  const parts = [
    `hit=${hit ? `${hit.line}:${hit.byteInLine}` : "null"}`,
    `over=idx${overIndex}/data${over?.dataset.line ?? "?"}`,
    overRect ? `overY=${Math.round(overRect.top)}-${Math.round(overRect.bottom)}` : "overY=?",
    roundTrip,
    caretRect ? `caretAt=${Math.round(caretRect.left)},${Math.round(caretRect.top)}` : "caretAt=none",
    highlight ? `selAt=${Math.round(highlight.left)},${Math.round(highlight.top)}` : "selAt=none",
  ];
  // A mismatch between a child's own index and its data-line means the two
  // indexing schemes the editor uses have diverged.
  if (over && String(overIndex) !== over.dataset.line) parts.push("INDEX-MISMATCH");
  return parts.join(" ");
}

/** Overshoot only means anything once the textarea cannot scroll further. */
function atScrollFloor() {
  const view = scroller();
  return view.scrollTop >= view.scrollHeight - view.clientHeight - 2;
}

function engineSelectionText() {
  const selection = editor.snapshot().selections[0];
  if (!selection) return "none";
  const { anchor, head } = selection;
  return `${anchor.line}:${anchor.byteInLine}->${head.line}:${head.byteInLine}`;
}

/**
 * Listeners run in the bubble phase deliberately: the component registered its
 * own handlers first, so by the time these fire `defaultPrevented` and the
 * selection state reflect what the editor decided to do with the event.
 */
const note = (label, extra = "") => {
  if (!traceEnabled) return;
  const at = String(Math.round(performance.now() - traceStart)).padStart(5, " ");
  trace.push(`${at} ${label.padEnd(22, " ")} ta=${input.selectionStart}..${input.selectionEnd}`
    + ` eng=${engineSelectionText()}(${engineCaretOffset()})`
    + `${extra ? ` ${extra}` : ""}`);
  if (trace.length > TRACE_LIMIT) trace.shift();
};

let lastMoveNote = 0;
for (const type of ["pointerdown", "pointermove", "pointerup", "pointercancel", "click"]) {
  for (const target of [input, shield].filter(Boolean)) target.addEventListener(type, (event) => {
    if (!event.isPrimary && type !== "click") return;
    // Stationary fingers emit a move every ~8ms and flooded the buffer, pushing
    // the interesting events out. Sample them; keep every other event.
    if (type === "pointermove") {
      const now = performance.now();
      if (now - lastMoveNote < 120 && !event.defaultPrevented) return;
      lastMoveNote = now;
    }
    // `PREVENTED` on a pointermove is the tell that the projection-owned
    // long-press drag has claimed the finger.
    const point = `@${Math.round(event.clientX)},${Math.round(event.clientY)}`;
    const detail = type === "pointerdown" || type === "click" || event.defaultPrevented
      ? ` ${probePoint(event.clientX, event.clientY)}`
      : "";
    note(`${type}(${event.pointerType ?? "?"})`,
      `${point}${event.defaultPrevented ? " PREVENTED" : ""}${detail}`);
  });
}
(shield ?? input).addEventListener("contextmenu", (event) => note(
  "contextmenu", `@${Math.round(event.clientX)},${Math.round(event.clientY)}`
    + `${event.defaultPrevented ? " CLAIMED" : " not-claimed"}`,
));
input.addEventListener("select", () => {
  // Where the first painted highlight row landed, so a drawing fault is
  // distinguishable from a mapping fault without another round trip.
  const rect = editor.shadowRoot.querySelector(".visual-selection")?.getBoundingClientRect();
  note("select", rect ? `selAt=${Math.round(rect.left)},${Math.round(rect.top)}` : "selAt=none");
});
input.addEventListener("compositionstart", () => note("compositionSTART"));
input.addEventListener("compositionupdate", (event) => note("compositionUPDATE", JSON.stringify(event.data ?? "")));
input.addEventListener("compositionend", (event) => note("compositionEND", JSON.stringify(event.data ?? "")));
input.addEventListener("beforeinput", (event) => note("beforeinput", event.inputType));
input.addEventListener("input", (event) => note("input", event.inputType ?? ""));
input.addEventListener("scroll", () => note("scroll", `top=${Math.round(input.scrollTop)}`));
document.addEventListener("selectionchange", () => note("selectionchange"));

// Clipboard outcomes are invisible otherwise: a rejected write and a successful
// one look identical on screen, so surface what the editor reported.
let clipboardStatus = "clipboard: (none yet)";
editor.addEventListener("continuity-request", (event) => {
  clipboardStatus = `clipboard: host-request ${event.detail.kind}`;
  note("request", event.detail.kind);
  render();
});
editor.addEventListener("continuity-error", (event) => {
  clipboardStatus = `clipboard: ERROR ${String(event.detail.error)}`;
  note("editor-error", String(event.detail.error).slice(0, 60));
  render();
});
// Whether the clipboard is blocked by the embedding rather than by the editor.
let clipboardContext = "";
(async () => {
  const framed = window.self !== window.top;
  let permission = "unknown";
  try {
    permission = (await navigator.permissions?.query({ name: "clipboard-read" }))?.state ?? "n/a";
  } catch (error) { permission = `query-failed(${String(error).slice(0, 30)})`; }
  clipboardContext = `embed: iframe=${framed} secure=${isSecureContext}`
    + ` readApi=${Boolean(navigator.clipboard?.readText)} permission=${permission}`;
  render();
})();
for (const id of ["copy", "cut", "paste", "select-all"]) {
  editor.shadowRoot.addEventListener("click", (event) => {
    if (event.target?.dataset?.selectionAction !== id) return;
    clipboardStatus = `clipboard: ${id} tapped, secure=${isSecureContext}`
      + ` api=${Boolean(navigator.clipboard?.writeText)}`;
    setTimeout(render, 60);
  });
}

document.querySelector("#trace").addEventListener("click", (event) => {
  traceEnabled = !traceEnabled;
  event.currentTarget.classList.toggle("on", traceEnabled);
  trace.length = 0;
  traceStart = performance.now();
  render();
});

document.querySelector("#clear").addEventListener("click", () => {
  trace.length = 0;
  traceStart = performance.now();
  render();
});

/** Everything needed to reason about a report, not just the event list. */
function buildTraceReport() {
  const frameBounds = frame.getBoundingClientRect();
  const children = projection.children;
  const last = children[children.length - 1];
  const overshoot = last ? Math.round(last.getBoundingClientRect().bottom - frameBounds.bottom) : 0;
  return [
    `build=${build} corpus=${document.querySelector("#corpus").value}`,
    `ua=${navigator.userAgent}`,
    `viewport=${innerWidth}x${innerHeight} dpr=${devicePixelRatio}`,
    `frame=${Math.round(frameBounds.width)}x${Math.round(frameBounds.height)}`,
    `theme=${editor.getAttribute("theme")} composing=${editor.composing}`,
    `range engine=${engineRange()?.join("..") ?? "none"}`
      + ` textarea=${input.selectionStart}..${input.selectionEnd}`
      + ` ${selectionAgrees() ? "agree" : "DRIFT"}`,
    `sel ta=${input.selectionStart}..${input.selectionEnd} dir=${input.selectionDirection}`
      + ` eng=${engineSelectionText()}`,
    `selText=${JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd).slice(0, 60))}`,
    `scroll top=${Math.round(scroller().scrollTop)}`
      + `/${Math.round(scroller().scrollHeight - scroller().clientHeight)}`
      + ` extent=${scroller().scrollHeight} proj=${projection.offsetHeight}`
      + ` via=${scroller() === input ? "textarea" : "shield"}`
      + ` pad=${input.dataset.scrollExtentPadding ?? 0}`
      + ` residual=${input.dataset.scrollExtentResidual ?? 0}`
      + ` overshoot=${overshoot}${atScrollFloor() ? " (at floor)" : " (not at floor, ignore)"}`,
    `lines=${children.length} chars=${editor.value.length}`,
    clipboardStatus,
    clipboardContext,
    "--- trace ---",
    ...trace,
  ].join("\n");
}

const fallback = document.querySelector("#fallback");
const fallbackText = document.querySelector("#fallback-text");
document.querySelector("#fallback-close").addEventListener("click", () => {
  fallback.classList.remove("open");
});

document.querySelector("#copy").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  const report = buildTraceReport();
  const done = (label) => {
    button.textContent = label;
    setTimeout(() => { button.textContent = "copy trace"; }, 1600);
  };
  // Secure-context path. Absent over plain HTTP, which is how the LAN and
  // Tailscale URLs are usually reached.
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(report);
      done("copied ✓");
      return;
    } catch { /* fall through to the legacy path */ }
  }
  // Legacy path: a real, visible, selected textarea. iOS ignores execCommand on
  // a hidden or readonly-selected node, so this one is on-screen and selected
  // via setSelectionRange.
  fallbackText.value = report;
  fallback.classList.add("open");
  fallbackText.focus();
  fallbackText.setSelectionRange(0, report.length);
  try {
    if (document.execCommand("copy")) {
      fallback.classList.remove("open");
      done("copied ✓");
      return;
    }
  } catch { /* leave the panel open for a manual copy */ }
  done("select & copy");
});

/** The readout is built with innerHTML for its status colouring; document text
 *  reaches it through the selection and composition fields, so escape it. */
function escapeHtml(value) {
  return String(value).replace(/[&<>]/gu, (character) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;" })[character]);
}

function positionToOffset(lines, position) {
  let offset = 0;
  for (let index = 0; index < position.line && index < lines.length; index += 1) {
    offset += lines[index].length + 1;
  }
  return offset + position.byteInLine;
}

/** Source offset of the engine's primary caret head. */
function engineCaretOffset() {
  const snapshot = editor.snapshot();
  const head = snapshot.selections[0]?.head;
  return head ? positionToOffset(snapshot.text.split("\n"), head) : null;
}

/**
 * The engine's primary selection as an ordered offset pair, so it can be
 * compared against the textarea's own range. Comparing only the head against
 * `selectionStart` reports a false drift for every non-collapsed selection.
 */
function engineRange() {
  const snapshot = editor.snapshot();
  const selection = snapshot.selections[0];
  if (!selection) return null;
  const lines = snapshot.text.split("\n");
  const anchor = positionToOffset(lines, selection.anchor);
  const head = positionToOffset(lines, selection.head);
  return [Math.min(anchor, head), Math.max(anchor, head)];
}

/** Whether the drawn selection and the textarea agree about the same range. */
function selectionAgrees() {
  const range = engineRange();
  return Boolean(range)
    && range[0] === input.selectionStart
    && range[1] === input.selectionEnd;
}

function render() {
  if (!showDiagnostics) return;
  const frameBounds = frame.getBoundingClientRect();
  const children = projection.children;
  const last = children[children.length - 1];
  const lastBounds = last ? last.getBoundingClientRect() : null;
  const overshoot = lastBounds ? Math.round(lastBounds.bottom - frameBounds.bottom) : 0;
  const atFloor = input.scrollTop >= input.scrollHeight - input.clientHeight - 2;

  const padding = input.dataset.scrollExtentPadding ?? "0";
  const residual = input.dataset.scrollExtentResidual ?? "0";
  const range = engineRange();
  const agree = selectionAgrees();

  // The tail is only provably reachable once the textarea is at its floor.
  const tailVerdict = !atFloor
    ? "scroll to end to test"
    : overshoot > 1
      ? `<span class="bad">UNREACHABLE by ${overshoot}px</span>`
      : '<span class="good">reachable</span>';

  readout.innerHTML = [
    `range  engine=${range ? `${range[0]}..${range[1]}` : "none"}`
      + ` textarea=${input.selectionStart}..${input.selectionEnd} `
      + (agree ? '<span class="good">agree</span>' : '<span class="bad">DRIFT</span>'),
    `sel    ${input.selectionStart}..${input.selectionEnd} `
      + escapeHtml(JSON.stringify(input.value.slice(input.selectionStart, input.selectionEnd).slice(0, 28))),
    `scroll top=${Math.round(scroller().scrollTop)}`
      + `/${Math.round(scroller().scrollHeight - scroller().clientHeight)}`
      + ` extent=${scroller().scrollHeight} proj=${projection.offsetHeight}`
      + ` via=${scroller() === input ? "textarea" : "shield"}`,
    `extent pad=${padding} residual=${residual}  tail: ${tailVerdict}`,
    `lines  ${children.length}  chars ${editor.value.length}`
      + `  composing=${editor.composing}`,
    escapeHtml(clipboardStatus),
    escapeHtml(clipboardContext),
    traceEnabled
      ? `--- trace (${trace.length}) ---\n${escapeHtml(trace.slice(-12).join("\n"))}`
      : "",
  ].filter(Boolean).join("\n");
}

for (const type of ["input", "select", "scroll", "click", "keyup", "pointerup"]) {
  input.addEventListener(type, () => requestAnimationFrame(render));
}
document.addEventListener("selectionchange", () => requestAnimationFrame(render));
editor.addEventListener("continuity-frame", () => requestAnimationFrame(render));
setInterval(render, 500);
render();
