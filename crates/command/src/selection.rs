//! Selection and multi-cursor commands.

use std::sync::Arc;

use crate::{CommandId, ContextPredicate, Registry};

/// Extend the active selection one character forward.
pub const EDITOR_EXTEND_CHAR_FORWARD: CommandId = CommandId("editor.extend_char_forward");
/// Extend the active selection one character backward.
pub const EDITOR_EXTEND_CHAR_BACKWARD: CommandId = CommandId("editor.extend_char_backward");
/// Extend the active selection one line up.
pub const EDITOR_EXTEND_LINE_UP: CommandId = CommandId("editor.extend_line_up");
/// Extend the active selection one line down.
pub const EDITOR_EXTEND_LINE_DOWN: CommandId = CommandId("editor.extend_line_down");
/// Extend the active selection to line start.
pub const EDITOR_EXTEND_LINE_START: CommandId = CommandId("editor.extend_line_start");
/// Extend the active selection to line end.
pub const EDITOR_EXTEND_LINE_END: CommandId = CommandId("editor.extend_line_end");
/// Extend the active selection to document start.
pub const EDITOR_EXTEND_DOC_START: CommandId = CommandId("editor.extend_doc_start");
/// Extend the active selection to document end.
pub const EDITOR_EXTEND_DOC_END: CommandId = CommandId("editor.extend_doc_end");
/// Add a cursor above the primary cursor.
pub const EDITOR_ADD_CURSOR_ABOVE: CommandId = CommandId("editor.add_cursor_above");
/// Add a cursor below the primary cursor.
pub const EDITOR_ADD_CURSOR_BELOW: CommandId = CommandId("editor.add_cursor_below");
/// Add a cursor at the next match of the selected text.
pub const EDITOR_ADD_CURSOR_AT_NEXT_MATCH: CommandId = CommandId("editor.add_cursor_at_next_match");
/// Add cursors at every match of the selected text.
pub const EDITOR_ADD_CURSOR_AT_ALL_MATCHES: CommandId =
    CommandId("editor.add_cursor_at_all_matches");
/// Extend a block selection upward.
pub const EDITOR_COLUMN_SELECT_UP: CommandId = CommandId("editor.column_select_up");
/// Extend a block selection downward.
pub const EDITOR_COLUMN_SELECT_DOWN: CommandId = CommandId("editor.column_select_down");
/// Clear secondary cursors.
pub const EDITOR_CLEAR_SECONDARY_CURSORS: CommandId = CommandId("editor.clear_secondary_cursors");
/// Select the word at each active selection.
pub const EDITOR_SELECT_WORD: CommandId = CommandId("editor.select_word");
/// Select the line at each active selection.
pub const EDITOR_SELECT_LINE: CommandId = CommandId("editor.select_line");
/// Select the paragraph at each active selection.
pub const EDITOR_SELECT_PARAGRAPH: CommandId = CommandId("editor.select_paragraph");
/// Expand selection smartly.
pub const EDITOR_EXPAND_SELECTION_SMART: CommandId = CommandId("editor.expand_selection_smart");
/// Select the entire buffer.
pub const EDITOR_SELECT_ALL: CommandId = CommandId("editor.select_all");
/// G5: drop selections whose text doesn't match a regex argument.
pub(crate) const SELECTION_KEEP_MATCHING: CommandId = CommandId("selection.keep_matching");
/// G5: drop selections whose text matches a regex argument.
pub(crate) const SELECTION_DISCARD_MATCHING: CommandId = CommandId("selection.discard_matching");
/// G5: break each selection into sub-selections at every regex match.
pub(crate) const SELECTION_SPLIT_ON: CommandId = CommandId("selection.split_on");

/// Register Phase 5 selection and multi-cursor commands.
pub fn register_selection_commands(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    registry.register(
        EDITOR_EXTEND_CHAR_FORWARD,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_char(1)),
    );
    registry.register(
        EDITOR_EXTEND_CHAR_BACKWARD,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_char(-1)),
    );
    registry.register(
        EDITOR_EXTEND_LINE_UP,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_line(-1)),
    );
    registry.register(
        EDITOR_EXTEND_LINE_DOWN,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_line(1)),
    );
    registry.register(
        EDITOR_EXTEND_LINE_START,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_line_start()),
    );
    registry.register(
        EDITOR_EXTEND_LINE_END,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_line_end()),
    );
    registry.register(
        EDITOR_EXTEND_DOC_START,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_doc_start()),
    );
    registry.register(
        EDITOR_EXTEND_DOC_END,
        focused.clone(),
        Arc::new(|_, ctx| ctx.extend_doc_end()),
    );
    registry.register(
        EDITOR_ADD_CURSOR_ABOVE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.add_cursor_above()),
    );
    registry.register(
        EDITOR_ADD_CURSOR_BELOW,
        focused.clone(),
        Arc::new(|_, ctx| ctx.add_cursor_below()),
    );
    registry.register(
        EDITOR_ADD_CURSOR_AT_NEXT_MATCH,
        focused.clone(),
        Arc::new(|_, ctx| ctx.add_cursor_at_next_match()),
    );
    registry.register(
        EDITOR_ADD_CURSOR_AT_ALL_MATCHES,
        focused.clone(),
        Arc::new(|_, ctx| ctx.add_cursor_at_all_matches()),
    );
    registry.register(
        EDITOR_COLUMN_SELECT_UP,
        focused.clone(),
        Arc::new(|_, ctx| ctx.column_select_up()),
    );
    registry.register(
        EDITOR_COLUMN_SELECT_DOWN,
        focused.clone(),
        Arc::new(|_, ctx| ctx.column_select_down()),
    );
    registry.register(
        EDITOR_CLEAR_SECONDARY_CURSORS,
        focused.clone(),
        Arc::new(|_, ctx| ctx.clear_secondary_cursors()),
    );
    registry.register(
        EDITOR_SELECT_WORD,
        focused.clone(),
        Arc::new(|_, ctx| ctx.select_word()),
    );
    registry.register(
        EDITOR_SELECT_LINE,
        focused.clone(),
        Arc::new(|_, ctx| ctx.select_line()),
    );
    registry.register(
        EDITOR_SELECT_PARAGRAPH,
        focused.clone(),
        Arc::new(|_, ctx| ctx.select_paragraph()),
    );
    registry.register(
        EDITOR_EXPAND_SELECTION_SMART,
        focused.clone(),
        Arc::new(|_, ctx| ctx.expand_selection_smart()),
    );
    registry.register(
        EDITOR_SELECT_ALL,
        focused.clone(),
        Arc::new(|_, ctx| ctx.select_all()),
    );

    // G5 selection arithmetic. Each command pulls its regex from
    // `args.as_str()` (the transient input bar threads it through
    // the registry's dispatch path) and routes to a single
    // `selection_arithmetic(op, regex)` Context method.
    for (cmd, op) in [
        (SELECTION_KEEP_MATCHING, "keep"),
        (SELECTION_DISCARD_MATCHING, "discard"),
        (SELECTION_SPLIT_ON, "split"),
    ] {
        registry.register(
            cmd,
            focused.clone(),
            Arc::new(move |args, ctx| {
                let regex = args.as_str().unwrap_or("");
                ctx.selection_arithmetic(op, regex)
            }),
        );
    }
}
