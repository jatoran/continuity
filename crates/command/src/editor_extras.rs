//! Phase 6 rich-editing command registration.
//!
//! Every handler collapses to `Context::apply_selection_edit`, so the
//! trait surface stays small. Cursor-motion extras (word, paragraph,
//! shrink-smart) live in [`crate::motion_extras`].

use std::sync::Arc;

use continuity_core::{CaseKind, IndentUnit, LineEnding, SelectionEdit, SortKind};
use serde_json::Value;

use crate::{CommandId, ContextPredicate, Error, Registry};

/// Insert a fresh line above each selection's line.
pub const EDITOR_INSERT_NEWLINE_ABOVE: CommandId = CommandId("editor.insert_newline_above");
/// Insert a fresh line below each selection's line.
pub const EDITOR_INSERT_NEWLINE_BELOW: CommandId = CommandId("editor.insert_newline_below");
/// Insert a newline that inherits the leading indent of the current line.
pub const EDITOR_INSERT_NEWLINE_SMART: CommandId = CommandId("editor.insert_newline_smart");
/// Toggle a `- ` bullet right after the leading whitespace of each line;
/// the caret stays on the same content character (its byte column shifts
/// by `±2` so the cursor is visually unchanged).
pub(crate) const EDITOR_TOGGLE_BULLET_AT_LINE_START: CommandId =
    CommandId("editor.toggle_bullet_at_line_start");
/// Delete from the caret to the previous word boundary.
pub const EDITOR_DELETE_WORD_BACKWARD: CommandId = CommandId("editor.delete_word_backward");
/// Delete from the caret to the next word boundary.
pub const EDITOR_DELETE_WORD_FORWARD: CommandId = CommandId("editor.delete_word_forward");
/// Delete from the caret to the start of the current line.
pub const EDITOR_DELETE_TO_LINE_START: CommandId = CommandId("editor.delete_to_line_start");
/// Delete from the caret to the end of the current line.
pub const EDITOR_DELETE_TO_LINE_END: CommandId = CommandId("editor.delete_to_line_end");
/// Delete from the caret to the matching bracket.
pub const EDITOR_DELETE_TO_BRACKET: CommandId = CommandId("editor.delete_to_bracket");
/// Duplicate each selected line.
pub const EDITOR_DUPLICATE_LINE: CommandId = CommandId("editor.duplicate_line");
/// Duplicate each non-empty selection's bytes.
pub const EDITOR_DUPLICATE_SELECTION: CommandId = CommandId("editor.duplicate_selection");
/// Move each selected line up one position.
pub const EDITOR_MOVE_LINE_UP_BLOCK: CommandId = CommandId("editor.move_line_up_block");
/// Move each selected line down one position.
pub const EDITOR_MOVE_LINE_DOWN_BLOCK: CommandId = CommandId("editor.move_line_down_block");
/// Join the line below each caret into the current line.
pub const EDITOR_JOIN_LINES: CommandId = CommandId("editor.join_lines");
/// Sort covered lines ascending, case-sensitive.
pub const EDITOR_SORT_LINES_ASC: CommandId = CommandId("editor.sort_lines_asc");
/// Sort covered lines descending, case-sensitive.
pub const EDITOR_SORT_LINES_DESC: CommandId = CommandId("editor.sort_lines_desc");
/// Sort covered lines ascending, case-insensitive.
pub const EDITOR_SORT_LINES_ASC_CI: CommandId = CommandId("editor.sort_lines_asc_ci");
/// Sort covered lines descending, case-insensitive.
pub const EDITOR_SORT_LINES_DESC_CI: CommandId = CommandId("editor.sort_lines_desc_ci");
/// Sort covered lines ascending by length.
pub const EDITOR_SORT_LINES_ASC_LEN: CommandId = CommandId("editor.sort_lines_asc_len");
/// Sort covered lines descending by length.
pub const EDITOR_SORT_LINES_DESC_LEN: CommandId = CommandId("editor.sort_lines_desc_len");
/// Sort covered lines ascending, natural-numeric.
pub const EDITOR_SORT_LINES_ASC_NUM: CommandId = CommandId("editor.sort_lines_asc_num");
/// Sort covered lines descending, natural-numeric.
pub const EDITOR_SORT_LINES_DESC_NUM: CommandId = CommandId("editor.sort_lines_desc_num");
/// Reverse the order of covered lines.
pub const EDITOR_REVERSE_LINES: CommandId = CommandId("editor.reverse_lines");
/// Drop duplicate covered lines.
pub const EDITOR_UNIQUE_LINES: CommandId = CommandId("editor.unique_lines");
/// Pseudo-randomly shuffle covered lines.
pub const EDITOR_SHUFFLE_LINES: CommandId = CommandId("editor.shuffle_lines");
/// Trim trailing whitespace from each covered line.
pub const EDITOR_TRIM_TRAILING_WHITESPACE: CommandId = CommandId("editor.trim_trailing_whitespace");
/// Hard-wrap covered paragraphs at column 80.
pub const EDITOR_WRAP_AT_COLUMN: CommandId = CommandId("editor.wrap_at_column");
/// Reflow covered paragraphs at column 80, preserving leading indent.
pub const EDITOR_REFLOW_PARAGRAPH: CommandId = CommandId("editor.reflow_paragraph");
/// Transpose two characters straddling each caret.
pub const EDITOR_TRANSPOSE_CHARS: CommandId = CommandId("editor.transpose_chars");
/// Transpose the two words straddling each caret.
pub const EDITOR_TRANSPOSE_WORDS: CommandId = CommandId("editor.transpose_words");
/// Upper-case selection bytes.
pub const EDITOR_CHANGE_CASE_UPPER: CommandId = CommandId("editor.change_case_upper");
/// Lower-case selection bytes.
pub const EDITOR_CHANGE_CASE_LOWER: CommandId = CommandId("editor.change_case_lower");
/// Title-case selection bytes.
pub const EDITOR_CHANGE_CASE_TITLE: CommandId = CommandId("editor.change_case_title");
/// Toggle the case of each ASCII letter in the selection.
pub const EDITOR_CHANGE_CASE_TOGGLE: CommandId = CommandId("editor.change_case_toggle");
/// Sentence-case selection bytes.
pub const EDITOR_CHANGE_CASE_SENTENCE: CommandId = CommandId("editor.change_case_sentence");
/// Indent each covered line by 4 spaces.
pub const EDITOR_INDENT: CommandId = CommandId("editor.indent");
/// Outdent each covered line by 4 spaces.
pub const EDITOR_OUTDENT: CommandId = CommandId("editor.outdent");
/// Replace 4-space runs with tabs on each covered line.
pub const EDITOR_SPACES_TO_TABS: CommandId = CommandId("editor.spaces_to_tabs");
/// Replace each tab with 4 spaces on each covered line.
pub const EDITOR_TABS_TO_SPACES: CommandId = CommandId("editor.tabs_to_spaces");
/// Convert covered line endings to LF.
pub const EDITOR_CONVERT_LINE_ENDINGS_LF: CommandId = CommandId("editor.convert_line_endings_lf");
/// Convert covered line endings to CRLF.
pub const EDITOR_CONVERT_LINE_ENDINGS_CRLF: CommandId =
    CommandId("editor.convert_line_endings_crlf");
/// Wrap each non-empty selection in `(`/`)`.
pub const EDITOR_SURROUND_PARENS: CommandId = CommandId("editor.surround_parens");
/// Wrap each non-empty selection in `[`/`]`.
pub const EDITOR_SURROUND_BRACKETS: CommandId = CommandId("editor.surround_brackets");
/// Wrap each non-empty selection in `{`/`}`.
pub const EDITOR_SURROUND_BRACES: CommandId = CommandId("editor.surround_braces");
/// Wrap each non-empty selection in `"`/`"`.
pub const EDITOR_SURROUND_DOUBLE_QUOTES: CommandId = CommandId("editor.surround_double_quotes");
/// Wrap each non-empty selection with the JSON `surround.with` argument.
pub const EDITOR_SURROUND_SELECTION_WITH: CommandId = CommandId("editor.surround_selection_with");

const DEFAULT_TAB_WIDTH: u32 = 4;
const DEFAULT_WRAP_COLUMN: u32 = 80;
const DEFAULT_SHUFFLE_SEED: u64 = 0xCAFE_F00D;

fn handler<F>(f: F) -> crate::registry::Handler
where
    F: Fn() -> SelectionEdit + Send + Sync + 'static,
{
    Arc::new(move |_, ctx| ctx.apply_selection_edit(f()))
}

/// Register Phase 6 rich-editing commands.
pub fn register_rich_editing(registry: &mut Registry) {
    let focused = ContextPredicate::parse("editor.focused");
    let bind = |registry: &mut Registry, id: CommandId, h: crate::registry::Handler| {
        registry.register(id, focused.clone(), h);
    };
    bind(
        registry,
        EDITOR_INSERT_NEWLINE_ABOVE,
        handler(|| SelectionEdit::InsertNewlineAbove),
    );
    bind(
        registry,
        EDITOR_INSERT_NEWLINE_BELOW,
        handler(|| SelectionEdit::InsertNewlineBelow),
    );
    bind(
        registry,
        EDITOR_INSERT_NEWLINE_SMART,
        handler(|| SelectionEdit::InsertNewlineSmart),
    );
    bind(
        registry,
        EDITOR_TOGGLE_BULLET_AT_LINE_START,
        handler(|| SelectionEdit::ToggleBulletAtLineStart),
    );
    bind(
        registry,
        EDITOR_DELETE_WORD_BACKWARD,
        handler(|| SelectionEdit::DeleteWordBackward),
    );
    bind(
        registry,
        EDITOR_DELETE_WORD_FORWARD,
        handler(|| SelectionEdit::DeleteWordForward),
    );
    bind(
        registry,
        EDITOR_DELETE_TO_LINE_START,
        handler(|| SelectionEdit::DeleteToLineStart),
    );
    bind(
        registry,
        EDITOR_DELETE_TO_LINE_END,
        handler(|| SelectionEdit::DeleteToLineEnd),
    );
    bind(
        registry,
        EDITOR_DELETE_TO_BRACKET,
        handler(|| SelectionEdit::DeleteToBracket),
    );
    bind(
        registry,
        EDITOR_DUPLICATE_LINE,
        handler(|| SelectionEdit::DuplicateLine),
    );
    bind(
        registry,
        EDITOR_DUPLICATE_SELECTION,
        handler(|| SelectionEdit::DuplicateSelection),
    );
    bind(
        registry,
        EDITOR_MOVE_LINE_UP_BLOCK,
        handler(|| SelectionEdit::MoveLineUp),
    );
    bind(
        registry,
        EDITOR_MOVE_LINE_DOWN_BLOCK,
        handler(|| SelectionEdit::MoveLineDown),
    );
    bind(
        registry,
        EDITOR_JOIN_LINES,
        handler(|| SelectionEdit::JoinLines),
    );

    for (id, kind) in [
        (EDITOR_SORT_LINES_ASC, SortKind::Asc),
        (EDITOR_SORT_LINES_DESC, SortKind::Desc),
        (EDITOR_SORT_LINES_ASC_CI, SortKind::AscCaseInsensitive),
        (EDITOR_SORT_LINES_DESC_CI, SortKind::DescCaseInsensitive),
        (EDITOR_SORT_LINES_ASC_LEN, SortKind::AscByLength),
        (EDITOR_SORT_LINES_DESC_LEN, SortKind::DescByLength),
        (EDITOR_SORT_LINES_ASC_NUM, SortKind::AscNumeric),
        (EDITOR_SORT_LINES_DESC_NUM, SortKind::DescNumeric),
    ] {
        bind(
            registry,
            id,
            handler(move || SelectionEdit::SortLines(kind)),
        );
    }

    bind(
        registry,
        EDITOR_REVERSE_LINES,
        handler(|| SelectionEdit::ReverseLines),
    );
    bind(
        registry,
        EDITOR_UNIQUE_LINES,
        handler(|| SelectionEdit::UniqueLines),
    );
    bind(
        registry,
        EDITOR_SHUFFLE_LINES,
        handler(|| SelectionEdit::ShuffleLines(DEFAULT_SHUFFLE_SEED)),
    );
    bind(
        registry,
        EDITOR_TRIM_TRAILING_WHITESPACE,
        // Phase B14: explicit `editor.trim_trailing_whitespace`
        // operates on the whole buffer, one undo group. The
        // selection-scoped variant remains available via
        // `SelectionEdit::TrimTrailingWhitespace` for internal use.
        handler(|| SelectionEdit::TrimTrailingWhitespaceAll),
    );
    bind(
        registry,
        EDITOR_WRAP_AT_COLUMN,
        handler(|| SelectionEdit::WrapAtColumn(DEFAULT_WRAP_COLUMN)),
    );
    bind(
        registry,
        EDITOR_REFLOW_PARAGRAPH,
        handler(|| SelectionEdit::ReflowParagraph(DEFAULT_WRAP_COLUMN)),
    );
    bind(
        registry,
        EDITOR_TRANSPOSE_CHARS,
        handler(|| SelectionEdit::TransposeChars),
    );
    bind(
        registry,
        EDITOR_TRANSPOSE_WORDS,
        handler(|| SelectionEdit::TransposeWords),
    );

    for (id, kind) in [
        (EDITOR_CHANGE_CASE_UPPER, CaseKind::Upper),
        (EDITOR_CHANGE_CASE_LOWER, CaseKind::Lower),
        (EDITOR_CHANGE_CASE_TITLE, CaseKind::Title),
        (EDITOR_CHANGE_CASE_TOGGLE, CaseKind::Toggle),
        (EDITOR_CHANGE_CASE_SENTENCE, CaseKind::Sentence),
    ] {
        bind(
            registry,
            id,
            handler(move || SelectionEdit::ChangeCase(kind)),
        );
    }

    bind(
        registry,
        EDITOR_INDENT,
        handler(|| SelectionEdit::Indent {
            unit: IndentUnit::Tab,
        }),
    );
    bind(
        registry,
        EDITOR_OUTDENT,
        handler(|| SelectionEdit::Outdent {
            unit: IndentUnit::Tab,
        }),
    );
    bind(
        registry,
        EDITOR_SPACES_TO_TABS,
        handler(|| SelectionEdit::SpacesToTabs {
            tab_width: DEFAULT_TAB_WIDTH,
        }),
    );
    bind(
        registry,
        EDITOR_TABS_TO_SPACES,
        handler(|| SelectionEdit::TabsToSpaces {
            tab_width: DEFAULT_TAB_WIDTH,
        }),
    );
    bind(
        registry,
        EDITOR_CONVERT_LINE_ENDINGS_LF,
        handler(|| SelectionEdit::ConvertLineEndings(LineEnding::Lf)),
    );
    bind(
        registry,
        EDITOR_CONVERT_LINE_ENDINGS_CRLF,
        handler(|| SelectionEdit::ConvertLineEndings(LineEnding::Crlf)),
    );

    for (id, open, close) in [
        (EDITOR_SURROUND_PARENS, "(", ")"),
        (EDITOR_SURROUND_BRACKETS, "[", "]"),
        (EDITOR_SURROUND_BRACES, "{", "}"),
        (EDITOR_SURROUND_DOUBLE_QUOTES, "\"", "\""),
    ] {
        bind(
            registry,
            id,
            handler(move || SelectionEdit::SurroundSelection {
                open: open.into(),
                close: close.into(),
            }),
        );
    }
    bind(
        registry,
        EDITOR_SURROUND_SELECTION_WITH,
        Arc::new(|args, ctx| {
            let (open, close) = surround_args(args)?;
            ctx.apply_selection_edit(SelectionEdit::SurroundSelection { open, close })
        }),
    );
}

fn surround_args(args: &Value) -> Result<(String, String), Error> {
    let invalid = |reason: &str| Error::InvalidArgs {
        name: EDITOR_SURROUND_SELECTION_WITH.as_str(),
        reason: reason.into(),
    };
    let obj = args
        .as_object()
        .ok_or_else(|| invalid("expected JSON object {open, close}"))?;
    let open = obj
        .get("open")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("`open` must be a string"))?;
    let close = obj
        .get("close")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("`close` must be a string"))?;
    Ok((open.to_string(), close.to_string()))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::Context;

    #[derive(Default)]
    struct Captor {
        last: Option<SelectionEdit>,
    }

    impl Context for Captor {
        fn lookup(&self, key: &str) -> Option<&str> {
            (key == "editor.focused").then_some("true")
        }
        fn apply_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
            self.last = Some(edit);
            Ok(())
        }
    }
    impl crate::ViewContext for Captor {}
    impl crate::FindContext for Captor {}

    fn registry() -> Registry {
        let mut registry = Registry::new();
        register_rich_editing(&mut registry);
        registry
    }

    type EditPredicate = fn(&SelectionEdit) -> bool;

    #[test]
    fn rich_editing_commands_emit_expected_edits() {
        let registry = registry();
        let mut ctx = Captor::default();
        let cases: &[(CommandId, EditPredicate)] = &[
            (EDITOR_INSERT_NEWLINE_ABOVE, |e| {
                matches!(e, SelectionEdit::InsertNewlineAbove)
            }),
            (EDITOR_INSERT_NEWLINE_BELOW, |e| {
                matches!(e, SelectionEdit::InsertNewlineBelow)
            }),
            (EDITOR_INSERT_NEWLINE_SMART, |e| {
                matches!(e, SelectionEdit::InsertNewlineSmart)
            }),
            (EDITOR_DELETE_WORD_BACKWARD, |e| {
                matches!(e, SelectionEdit::DeleteWordBackward)
            }),
            (EDITOR_DELETE_WORD_FORWARD, |e| {
                matches!(e, SelectionEdit::DeleteWordForward)
            }),
            (EDITOR_DELETE_TO_LINE_START, |e| {
                matches!(e, SelectionEdit::DeleteToLineStart)
            }),
            (EDITOR_DELETE_TO_LINE_END, |e| {
                matches!(e, SelectionEdit::DeleteToLineEnd)
            }),
            (EDITOR_DELETE_TO_BRACKET, |e| {
                matches!(e, SelectionEdit::DeleteToBracket)
            }),
            (EDITOR_DUPLICATE_LINE, |e| {
                matches!(e, SelectionEdit::DuplicateLine)
            }),
            (EDITOR_DUPLICATE_SELECTION, |e| {
                matches!(e, SelectionEdit::DuplicateSelection)
            }),
            (EDITOR_MOVE_LINE_UP_BLOCK, |e| {
                matches!(e, SelectionEdit::MoveLineUp)
            }),
            (EDITOR_MOVE_LINE_DOWN_BLOCK, |e| {
                matches!(e, SelectionEdit::MoveLineDown)
            }),
            (EDITOR_JOIN_LINES, |e| matches!(e, SelectionEdit::JoinLines)),
            (EDITOR_SORT_LINES_ASC, |e| {
                matches!(e, SelectionEdit::SortLines(SortKind::Asc))
            }),
            (EDITOR_SORT_LINES_ASC_NUM, |e| {
                matches!(e, SelectionEdit::SortLines(SortKind::AscNumeric))
            }),
            (EDITOR_REVERSE_LINES, |e| {
                matches!(e, SelectionEdit::ReverseLines)
            }),
            (EDITOR_UNIQUE_LINES, |e| {
                matches!(e, SelectionEdit::UniqueLines)
            }),
            (EDITOR_SHUFFLE_LINES, |e| {
                matches!(e, SelectionEdit::ShuffleLines(_))
            }),
            (EDITOR_TRIM_TRAILING_WHITESPACE, |e| {
                matches!(e, SelectionEdit::TrimTrailingWhitespaceAll)
            }),
            (EDITOR_WRAP_AT_COLUMN, |e| {
                matches!(e, SelectionEdit::WrapAtColumn(_))
            }),
            (EDITOR_REFLOW_PARAGRAPH, |e| {
                matches!(e, SelectionEdit::ReflowParagraph(_))
            }),
            (EDITOR_TRANSPOSE_CHARS, |e| {
                matches!(e, SelectionEdit::TransposeChars)
            }),
            (EDITOR_TRANSPOSE_WORDS, |e| {
                matches!(e, SelectionEdit::TransposeWords)
            }),
            (EDITOR_CHANGE_CASE_UPPER, |e| {
                matches!(e, SelectionEdit::ChangeCase(CaseKind::Upper))
            }),
            (EDITOR_CHANGE_CASE_TITLE, |e| {
                matches!(e, SelectionEdit::ChangeCase(CaseKind::Title))
            }),
            (EDITOR_INDENT, |e| matches!(e, SelectionEdit::Indent { .. })),
            (EDITOR_OUTDENT, |e| {
                matches!(e, SelectionEdit::Outdent { .. })
            }),
            (EDITOR_SPACES_TO_TABS, |e| {
                matches!(e, SelectionEdit::SpacesToTabs { .. })
            }),
            (EDITOR_TABS_TO_SPACES, |e| {
                matches!(e, SelectionEdit::TabsToSpaces { .. })
            }),
            (EDITOR_CONVERT_LINE_ENDINGS_LF, |e| {
                matches!(e, SelectionEdit::ConvertLineEndings(LineEnding::Lf))
            }),
            (EDITOR_SURROUND_PARENS, |e| {
                matches!(e, SelectionEdit::SurroundSelection { .. })
            }),
        ];
        for (id, predicate) in cases {
            registry
                .dispatch(*id, &Value::Null, &mut ctx)
                .expect("dispatch ok");
            let edit = ctx.last.as_ref().expect("edit captured");
            assert!(predicate(edit), "wrong edit for {}", id.as_str());
        }
    }

    #[test]
    fn surround_with_parses_args() {
        let registry = registry();
        let mut ctx = Captor::default();
        let args = serde_json::json!({"open": "<", "close": ">"});
        registry
            .dispatch(EDITOR_SURROUND_SELECTION_WITH, &args, &mut ctx)
            .expect("dispatch ok");
        match ctx.last.as_ref().expect("edit captured") {
            SelectionEdit::SurroundSelection { open, close } => {
                assert_eq!(open, "<");
                assert_eq!(close, ">");
            }
            other => panic!("wrong edit: {other:?}"),
        }
    }

    #[test]
    fn surround_with_rejects_invalid_args() {
        let registry = registry();
        let mut ctx = Captor::default();
        let err = registry
            .dispatch(EDITOR_SURROUND_SELECTION_WITH, &Value::Null, &mut ctx)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidArgs { .. }));
    }
}
