// Browser typography and textarea caret-position measurement. These read live
// layout from the DOM (canvas metrics and a hidden mirror element) and are used
// by scroll reveal, resize anchoring, and viewport persistence; kept apart from
// the projection renderer so each file owns one responsibility.

/** Average character width and line height for the element's computed font. */
export function measureBrowserTypography(element) {
  const style = getComputedStyle(element);
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (!context) {
    return { averageCharacterWidth: 0, lineHeight: parseFloat(style.lineHeight) || 0 };
  }
  context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  return {
    averageCharacterWidth: context.measureText("abcdefghijklmnopqrstuvwxyz").width / 26,
    lineHeight: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.5,
  };
}

/** Top pixel offset of the textarea caret via a hidden style-matched mirror. */
export function measureTextareaCaretTop(input, width = input.getBoundingClientRect().width) {
  const style = getComputedStyle(input);
  const caretOffset = input.selectionDirection === "backward"
    ? input.selectionStart
    : input.selectionEnd;
  const mirror = document.createElement("div");
  mirror.style.cssText = [
    "position:fixed",
    "inset:0 auto auto -10000px",
    "visibility:hidden",
    "pointer-events:none",
    "white-space:pre-wrap",
    "overflow-wrap:anywhere",
    "box-sizing:border-box",
  ].join(";");
  for (const property of [
    "font",
    "letterSpacing",
    "lineHeight",
    "padding",
    "border",
    "tabSize",
    "textIndent",
    "wordSpacing",
  ]) {
    mirror.style[property] = style[property];
  }
  mirror.style.width = `${width}px`;
  mirror.append(document.createTextNode(input.value.slice(0, caretOffset)));
  const caret = document.createElement("span");
  caret.textContent = input.value.slice(caretOffset, caretOffset + 1) || "​";
  mirror.append(caret);
  document.body.append(mirror);
  const top = caret.offsetTop;
  mirror.remove();
  return top;
}

/** Scroll the textarea minimally so the caret row stays within a one-row margin. */
export function revealTextareaCaret(input) {
  const lineHeight = measureBrowserTypography(input).lineHeight;
  const caretTop = measureTextareaCaretTop(input);
  const caretBottom = caretTop + lineHeight;
  const margin = Math.min(lineHeight, Math.max(0, (input.clientHeight - lineHeight) / 2));
  const visibleTop = input.scrollTop;
  const visibleBottom = visibleTop + input.clientHeight;
  if (caretTop < visibleTop + margin) {
    input.scrollTop = Math.max(0, caretTop - margin);
  } else if (caretBottom > visibleBottom - margin) {
    input.scrollTop = caretBottom - input.clientHeight + margin;
  }
  return input.scrollTop;
}
