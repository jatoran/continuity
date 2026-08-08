import { handleCut, handleDrop, handlePaste } from "./input_events.js";
import { handleTouchLongPress, isSurfaceEvent } from "./component_pointer.js";

/**
 * Clipboard, drop, and context-menu adapters for the editor element.
 *
 * Each one binds a browser transfer event to the shared pointer context: the
 * element only forwards, so the read-only policy and the host-request sequence
 * stay in one place instead of being restated per event.
 */
export function createTransferHandlers(ctx, host) {
  return {
    onPaste: (event) => handlePaste(event, host.readOnly, (text) => ctx.insertText(text)),

    onCut: (event) => handleCut(
      event,
      host.readOnly,
      ctx.editor()?.snapshot() ?? { text: ctx.input.value, selections: [] },
      ctx.input,
      () => ctx.insertText(""),
    ),

    onDrop: (event) => handleDrop(
      event,
      host.readOnly,
      (files) => ctx.emitRequest("filesDropped", { files }),
      (text) => ctx.insertText(text),
    ),

    onContextMenu: (event) => {
      if (!isSurfaceEvent(ctx, event)) return;
      // On touch this is the platform's long-press signal. Claim it so selection
      // stays on projected geometry; the host still hears the request either way.
      const isLongPressSelection = handleTouchLongPress(ctx, event);
      ctx.emitRequest("contextMenu", {
        clientX: event.clientX, clientY: event.clientY, isLongPressSelection,
      });
    },
  };
}
