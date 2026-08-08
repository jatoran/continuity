// Shared browser-test scaffolding for the coarse-pointer suites.
//
// Every touch assertion needs the same four things: an editor mounted and
// settled past the 250ms projection timer, synthetic pointer sequences that
// look like a finger rather than a mouse, the client rect of one *rendered*
// glyph, and the source byte that glyph came from. They live here so the suites
// that make those assertions stay under the file cap and cannot drift apart on
// what "settled" or "a tap" means.

export async function mountEditor(ContinuityEditorElement, mount, value, options = {}) {
  const editor = new ContinuityEditorElement();
  editor.setAttribute("aria-label", options.label ?? "Touch selection document");
  if (options.indentGuides) editor.setAttribute("indent-guides", options.indentGuides);
  if (options.theme) editor.setAttribute("theme", options.theme);
  if (options.commandRail) editor.setAttribute("command-rail", options.commandRail);
  editor.value = value;
  mount.append(editor);
  await editor.ready;
  editor.scrollIntoView({ block: "center" });
  const shadow = editor.shadowRoot;
  const input = shadow.querySelector("textarea");
  input.focus();
  await settle();
  return {
    editor,
    input,
    projection: shadow.querySelector(".projection"),
    frame: shadow.querySelector(".frame"),
    shield: shadow.querySelector(".touch-shield"),
    dispose: () => { editor.destroy(); editor.remove(); },
  };
}

/**
 * The Markdown projection lands on a 250ms idle timer after the fast keystroke
 * pass. Waiting only on animation frames asserts against undecorated raw source
 * and passes vacuously.
 */
export async function settle() {
  await animationFrames(2);
  await new Promise((resolve) => setTimeout(resolve, 320));
  await animationFrames(3);
}

/** Hold past the gesture's long-press threshold, then let the claim render. */
export async function longPress() {
  await new Promise((resolve) => setTimeout(resolve, 420));
  await settle();
}

/**
 * Drive the textarea to its scroll floor. Realizing projected detail changes
 * line heights and therefore the extent, so re-apply until it stops moving.
 */
export async function scrollToBottom(input) {
  let previous = -1;
  for (let attempt = 0; attempt < 15 && input.scrollTop !== previous; attempt += 1) {
    previous = input.scrollTop;
    input.scrollTop = input.scrollHeight;
    input.dispatchEvent(new Event("scroll"));
    await animationFrames(4);
  }
  await settle();
  input.scrollTop = input.scrollHeight;
  input.dispatchEvent(new Event("scroll"));
  await animationFrames(4);
}

export function animationFrames(count) {
  return new Promise((resolve) => {
    const next = () => (count-- <= 0 ? resolve() : requestAnimationFrame(next));
    requestAnimationFrame(next);
  });
}

export function pointerEvent(type, clientX, clientY, extra = {}) {
  return new PointerEvent(type, {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 21,
    isPrimary: true, button: 0, buttons: 1, clientX, clientY, ...extra,
  });
}

/** Dispatch the pointer sequence a finger tap produces. */
export function tap(input, clientX, clientY) {
  input.dispatchEvent(pointerEvent("pointerdown", clientX, clientY));
  input.dispatchEvent(pointerEvent("pointerup", clientX, clientY, { buttons: 0 }));
  input.dispatchEvent(new PointerEvent("click", {
    bubbles: true, cancelable: true, pointerType: "touch", pointerId: 21,
    button: 0, buttons: 0, clientX, clientY, detail: 1,
  }));
}

export function placeCaret(input, offset) {
  input.focus();
  input.setSelectionRange(offset, offset);
  input.dispatchEvent(new Event("select", { bubbles: true }));
}

/**
 * Swallow `select` so a caret move reaches the editor only through
 * `selectionchange`, which is what the platform does for a collapsed move.
 * The listener must sit on an ancestor in the capture phase: listeners on the
 * textarea itself run in registration order, and the editor registered first.
 */
export function suppressSelectEvent(root) {
  const suppress = (event) => event.stopImmediatePropagation();
  root.addEventListener("select", suppress, { capture: true });
  return () => root.removeEventListener("select", suppress, { capture: true });
}

/** Client rect of one rendered character across nested inline spans. */
export function glyphRect(element, charIndex) {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let consumed = 0;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (charIndex >= consumed && charIndex < consumed + node.length) {
      const range = document.createRange();
      range.setStart(node, charIndex - consumed);
      range.setEnd(node, charIndex - consumed + 1);
      const rects = [...range.getClientRects()].filter((rect) => rect.height > 0);
      return rects[0] ?? null;
    }
    consumed += node.length;
  }
  return null;
}

/**
 * Where a projected character lives in the source line. The projection folds
 * markers away, so the rendered string is a subsequence of the source; locating
 * the rendered prefix's tail inside the source gives the expected source byte
 * without duplicating the engine's own mapping tables.
 */
export function projectedCharToSourceByte(sourceLine, renderedLine, charIndex) {
  let sourceIndex = 0;
  for (const character of renderedLine.slice(0, charIndex)) {
    const found = sourceLine.indexOf(character, sourceIndex);
    if (found < 0) continue;
    sourceIndex = found + 1;
  }
  // Skip any folded marker run sitting between the prefix and the touched
  // glyph, so `**bold**` resolves to the `b`, not to the leading asterisks.
  const target = renderedLine[charIndex];
  const glyph = target === undefined ? -1 : sourceLine.indexOf(target, sourceIndex);
  if (glyph >= 0) sourceIndex = glyph;
  return new TextEncoder().encode(sourceLine.slice(0, sourceIndex)).byteLength;
}

/** UTF-16 index of a source-line byte offset, for measuring a rendered glyph. */
export function utf16IndexForByte(sourceLine, byteInLine) {
  const encoder = new TextEncoder();
  for (let index = 0; index <= sourceLine.length; index += 1) {
    if (encoder.encode(sourceLine.slice(0, index)).byteLength >= byteInLine) return index;
  }
  return sourceLine.length;
}
