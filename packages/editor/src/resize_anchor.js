import { measureBrowserTypography, measureTextareaCaretTop } from "./projection_measure.js";

/** Observe browser reflow while preserving the active caret's screen row. */
export function observeEditorResize(host, input, frame, callbacks) {
  let previousScrollHeight = 0;
  let hiddenScrollTop = null;
  const observer = new ResizeObserver(() => {
    const inputWidth = input.getBoundingClientRect().width;
    if (inputWidth === 0) {
      // The host hid the editor (display:none tab switch). Layout is gone and
      // the textarea scroll reads zero; latch the tracked viewport so re-show
      // restores it instead of snapping to the top.
      hiddenScrollTop ??= callbacks.scrollTop();
      return;
    }
    if (hiddenScrollTop !== null) {
      input.scrollTop = hiddenScrollTop;
      callbacks.setScrollTop(input.scrollTop);
      hiddenScrollTop = null;
    }
    const previousWidth = callbacks.inputWidth();
    if (callbacks.hasEditor() && previousWidth > 0 && inputWidth !== previousWidth) {
      if (input.selectionEnd === input.value.length && input.selectionStart === input.selectionEnd) {
        const bottomDistance = Math.max(0, previousScrollHeight - callbacks.scrollTop());
        input.scrollTop = Math.max(0, input.scrollHeight - bottomDistance);
      } else {
        const previousCaretTop = measureTextareaCaretTop(input, previousWidth);
        const caretScreenY = previousCaretTop - callbacks.scrollTop();
        const nextCaretTop = measureTextareaCaretTop(input, inputWidth);
        input.scrollTop = Math.max(0, nextCaretTop - caretScreenY);
      }
      callbacks.setScrollTop(input.scrollTop);
    } else if (callbacks.hasEditor() && Math.abs(input.scrollTop - callbacks.scrollTop()) > 1) {
      // Same-width reappearance after a hide: the browser zeroed the textarea
      // scroll while no layout existed. Reassert the tracked viewport.
      input.scrollTop = callbacks.scrollTop();
    }
    callbacks.setInputWidth(inputWidth);
    const metrics = measureBrowserTypography(input);
    frame.style.setProperty("--continuity-character-width", `${metrics.averageCharacterWidth}px`);
    frame.style.setProperty("--continuity-line-height", `${metrics.lineHeight}px`);
    previousScrollHeight = input.scrollHeight;
    callbacks.scheduleRender(performance.now());
  });
  observer.observe(host);
  return observer;
}
