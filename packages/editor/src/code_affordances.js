import { sourceLineStarts, utf8ByteToUtf16 } from "./coordinates.js";

/** Rebuild copy controls from canonical code spans (hover-revealed on fine
 * pointers; block controls stay visible on touch, where there is no hover). */
export function renderCodeAffordances(
  container,
  projectionContainer,
  snapshot,
  projection,
  activeLines,
  onCopy,
) {
  const lineStarts = sourceLineStarts(snapshot.text);
  const fragment = document.createDocumentFragment();
  const isTouchSurface = hasCoarsePointer();
  projection.blocks
    .filter(({ kind }) => kind === "fencedCodeBlock")
    .forEach((span) => {
      const startLine = lineForByte(lineStarts, span.startByte);
      const endLine = lineForByte(lineStarts, Math.max(span.startByte, span.endByte - 1));
      if ([...activeLines].some((line) => line >= startLine && line <= endLine)) {
        return;
      }
      const button = copyButton("code block", fencedCodeText(snapshot.text, span), onCopy);
      button.classList.add("code-copy-block");
      if (isTouchSurface) {
        // Touch has no hover: a tap-revealed control dies on the next render
        // rebuild, so the block control is permanently visible instead.
        button.classList.add("visible");
        button.dataset.alwaysVisible = "true";
      }
      button.style.top = `${(projectionContainer.children[startLine]?.offsetTop ?? 0) + 4}px`;
      button.continuityHitTest = (clientX, clientY) => hitLineRange(
        projectionContainer,
        startLine,
        endLine,
        clientX,
        clientY,
      );
      fragment.append(button);
    });

  projectionContainer.querySelectorAll(".inline-code[data-source-start]").forEach((span) => {
    const startByte = Number(span.dataset.sourceStart);
    const endByte = Number(span.dataset.sourceEnd);
    const button = copyButton(
      "inline code",
      inlineCodeText(sourceSlice(snapshot.text, startByte, endByte)),
      onCopy,
    );
    button.classList.add("code-copy-inline");
    button.style.left = `${span.offsetParent.offsetLeft + span.offsetLeft + span.offsetWidth + 4}px`;
    button.style.top = `${span.offsetParent.offsetTop + span.offsetTop}px`;
    button.continuityHitTest = (clientX, clientY) => [...span.getClientRects()]
      .some((rect) => pointInside(rect, clientX, clientY));
    fragment.append(button);
  });
  container.replaceChildren(fragment);
}

/** Show only the copy control whose code region is under the pointer. */
export function revealCodeAffordanceAt(container, clientX, clientY) {
  let revealed = false;
  container.querySelectorAll("button:not([data-always-visible])").forEach((button) => {
    const isButtonHit = pointInside(button.getBoundingClientRect(), clientX, clientY);
    const isCodeHit = button.continuityHitTest?.(clientX, clientY) ?? false;
    const isVisible = !revealed && (isButtonHit || isCodeHit);
    button.classList.toggle("visible", isVisible);
    revealed ||= isVisible;
  });
}

/** Hide pointer-revealed copy controls; permanently visible ones stay. */
export function hideCodeAffordances(container) {
  container.querySelectorAll("button:not([data-always-visible])")
    .forEach((button) => button.classList.remove("visible"));
}

/** Whether the primary pointer is a touch surface without reliable hover. */
export function hasCoarsePointer() {
  return globalThis.matchMedia?.("(pointer: coarse)").matches ?? false;
}

/** Write through the modern Clipboard API with a Chromium-compatible fallback. */
export async function writeClipboardText(text) {
  try {
    if (!navigator.clipboard) {
      // Insecure contexts have no Clipboard API; use the fallback directly.
      throw new Error("Clipboard API unavailable");
    }
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const fallback = document.createElement("textarea");
    fallback.value = text;
    // Read-only stops mobile keyboards from flashing open for the selection.
    fallback.readOnly = true;
    fallback.setAttribute("aria-hidden", "true");
    fallback.style.cssText = "position:fixed;left:-10000px;top:0";
    document.body.append(fallback);
    fallback.focus({ preventScroll: true });
    fallback.setSelectionRange(0, fallback.value.length);
    const didCopy = document.execCommand("copy");
    fallback.remove();
    if (!didCopy) {
      throw new Error("Browser clipboard write was rejected");
    }
  }
}

function copyButton(label, text, onCopy) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "code-copy";
  button.textContent = "Copy";
  button.setAttribute("aria-label", `Copy ${label}`);
  button.addEventListener("pointerdown", (event) => event.preventDefault());
  button.addEventListener("click", async () => {
    try {
      await onCopy(text);
      button.textContent = "Copied";
      button.classList.add("copied", "visible");
    } catch {
      button.textContent = "Copy failed";
      button.classList.add("visible");
    }
    setTimeout(() => {
      button.textContent = "Copy";
      button.classList.remove("copied");
    }, 1_500);
  });
  return button;
}

function inlineCodeText(source) {
  const match = source.match(/^(`+)([\s\S]*?)\1$/u);
  return match ? match[2].replace(/^ | $/gu, "") : source;
}

function hitLineRange(container, startLine, endLine, clientX, clientY) {
  for (let line = startLine; line <= endLine; line += 1) {
    const element = container.children[line];
    if (element && pointInside(element.getBoundingClientRect(), clientX, clientY)) {
      return true;
    }
  }
  return false;
}

function pointInside(rect, clientX, clientY) {
  return clientX >= rect.left && clientX <= rect.right
    && clientY >= rect.top && clientY <= rect.bottom;
}

function lineForByte(lineStarts, sourceByte) {
  let line = 0;
  for (let index = 1; index < lineStarts.length; index += 1) {
    if (lineStarts[index] > sourceByte) {
      break;
    }
    line = index;
  }
  return line;
}

function sourceSlice(text, startByte, endByte) {
  return text.slice(utf8ByteToUtf16(text, startByte), utf8ByteToUtf16(text, endByte));
}

function fencedCodeText(text, span) {
  const lines = sourceSlice(text, span.startByte, span.endByte).split("\n");
  if (/^\s*(`{3,}|~{3,})/u.test(lines[0] ?? "")) {
    lines.shift();
  }
  if (/^\s*(`{3,}|~{3,})\s*$/u.test(lines.at(-1) ?? "")) {
    lines.pop();
  }
  return lines.join("\n");
}
