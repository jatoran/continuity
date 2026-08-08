import { resolveEditorShortcutDecision } from "./shortcuts.js";

/** Normalize browser keyboard events into editor-owned operations or focus escape. */
export function handleEditorKeyDown(event, context) {
  if (context.isComposing) {
    return false;
  }

  const doesTabIndent = context.tabBehavior !== "focus" && !context.isReadOnly;
  if (event.key === "Escape") {
    if (context.hasMultipleSelections) {
      event.preventDefault();
      event.stopPropagation();
      context.clearSecondarySelections();
      return false;
    }
    return doesTabIndent;
  }
  const isVerticalCaretChord = event.altKey
    && (event.ctrlKey || event.metaKey)
    && !event.shiftKey
    && (event.key === "ArrowUp" || event.key === "ArrowDown");
  if (isVerticalCaretChord && !context.isReadOnly) {
    event.preventDefault();
    event.stopPropagation();
    context.addVerticalCaret(event.key === "ArrowUp" ? -1 : 1);
    return false;
  }
  const isUnmodifiedEnter = event.key === "Enter"
    && !event.ctrlKey && !event.metaKey && !event.altKey;
  if (isUnmodifiedEnter && !context.isReadOnly) {
    event.preventDefault();
    event.stopPropagation();
    if (event.shiftKey) {
      context.insertRawNewline();
    } else {
      context.executeCommand("editor.insert_newline_smart");
    }
    return false;
  }
  if (event.key !== "Tab") {
    if (!context.isReadOnly) {
      const decision = resolveEditorShortcutDecision(
        event,
        context.shortcutPolicy,
        context.shortcutBindings,
      );
      if (decision.kind === "command") {
        event.preventDefault();
        event.stopPropagation();
        context.executeCommand(decision.command);
      } else if (decision.kind === "suppressed") {
        context.onShortcutSuppressed(decision);
      }
    }
    return false;
  }

  const hasTraversalModifier = event.ctrlKey || event.metaKey || event.altKey;
  if (!doesTabIndent || context.isFocusEscapeArmed || hasTraversalModifier) {
    return false;
  }

  event.preventDefault();
  event.stopPropagation();
  context.executeCommand(event.shiftKey ? "editor.outdent" : "editor.indent");
  return false;
}

/** Accessible instructions for the active Tab policy. */
export function keyboardHelpText(tabBehavior, isReadOnly) {
  if (tabBehavior === "focus" || isReadOnly) {
    return "Tab moves focus out of the editor.";
  }
  return "Tab indents and Shift+Tab outdents. Press Escape, then Tab or Shift+Tab, to move focus out of the editor.";
}
