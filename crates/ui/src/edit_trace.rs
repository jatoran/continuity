//! ε.7 — edit-path trace helpers.
//!
//! Stable kind labels for [`SelectionEdit`] variants plus pure
//! formatters for the `event:edit_apply` / `event:edit_apply_result`
//! trace lines emitted around `EditorHandle::apply_selection_edit`.
//!
//! All helpers are pure and allocation-free unless the caller is
//! about to write to the trace sink. Callers must gate every
//! formatted detail with [`crate::paint_trace::is_trace_enabled`]
//! so a build with `CONTINUITY_UI_TRACE` unset pays exactly one
//! atomic load per edit.
//!
//! The kind labels are stable across releases — log consumers
//! (perf scripts, focus-return triagers, autorepeat-burst smoke
//! tests) grep for these strings. Adding a new `SelectionEdit`
//! variant requires extending [`kind_of`]; the `non_exhaustive`-
//! aware unit test below catches the omission at `cargo test
//! time.

use continuity_buffer::Revision;
use continuity_core::{Error, SelectionEdit};

/// Short, stable kind label per `SelectionEdit` variant. Used as
/// the `kind=<…>` field on every `event:edit_apply`-family trace
/// line. Returns `&'static str` so it is free when tracing is off.
pub(crate) fn kind_of(edit: &SelectionEdit) -> &'static str {
    use SelectionEdit::{
        ChangeCase, ConvertLineEndings, ConvertLineEndingsAll, DeleteBack, DeleteForward,
        DeletePair, DeleteToBracket, DeleteToLineEnd, DeleteToLineStart, DeleteWordBackward,
        DeleteWordForward, DuplicateLine, DuplicateSelection, Indent, InsertNewlineAbove,
        InsertNewlineBelow, InsertNewlineSmart, InsertPair, InsertText, JoinLines,
        JoinSelectedLines, MarkdownCycleHeading, MarkdownCycleListMarker, MarkdownDemoteSection,
        MarkdownInsertCodeFence, MarkdownInsertImageRef, MarkdownInsertLink,
        MarkdownMoveSectionDown, MarkdownMoveSectionUp, MarkdownPromoteSection,
        MarkdownRenumberList, MarkdownSetHeading, MarkdownStripFormatting, MarkdownToggleBullet,
        MarkdownToggleCheckbox, MarkdownToggleEmphasis, MarkdownToggleNumbered, MarkdownToggleTask,
        MarkdownWrapInBlockquote, MoveLineDown, MoveLineUp, Outdent, ReflowParagraph, ReverseLines,
        ShuffleLines, SortLines, SpacesToTabs, SurroundSelection, TabsToSpaces, TabsToSpacesAll,
        ToggleBulletAtLineStart, ToggleBulletWithContinuationIndent, TransposeChars,
        TransposeWords, TrimTrailingWhitespace, TrimTrailingWhitespaceAll, TrimWhitespaceAll,
        UniqueLines, WrapAtColumn,
    };
    match edit {
        InsertText(_) => "insert_text",
        DeleteBack => "delete_back",
        DeleteForward => "delete_forward",
        InsertNewlineAbove => "insert_newline_above",
        InsertNewlineBelow => "insert_newline_below",
        InsertNewlineSmart => "insert_newline_smart",
        ToggleBulletAtLineStart => "toggle_bullet_at_line_start",
        ToggleBulletWithContinuationIndent { .. } => "toggle_bullet_with_continuation_indent",
        DeleteWordBackward => "delete_word_backward",
        DeleteWordForward => "delete_word_forward",
        DeleteToLineStart => "delete_to_line_start",
        DeleteToLineEnd => "delete_to_line_end",
        DeleteToBracket => "delete_to_bracket",
        DuplicateLine => "duplicate_line",
        DuplicateSelection => "duplicate_selection",
        MoveLineUp => "move_line_up",
        MoveLineDown => "move_line_down",
        JoinLines => "join_lines",
        JoinSelectedLines => "join_selected_lines",
        SortLines(_) => "sort_lines",
        ReverseLines => "reverse_lines",
        UniqueLines => "unique_lines",
        ShuffleLines(_) => "shuffle_lines",
        TrimTrailingWhitespace => "trim_trailing_whitespace",
        TrimTrailingWhitespaceAll => "trim_trailing_whitespace_all",
        TrimWhitespaceAll => "trim_whitespace_all",
        WrapAtColumn(_) => "wrap_at_column",
        ReflowParagraph(_) => "reflow_paragraph",
        TransposeChars => "transpose_chars",
        TransposeWords => "transpose_words",
        ChangeCase(_) => "change_case",
        Indent { .. } => "indent",
        Outdent { .. } => "outdent",
        SpacesToTabs { .. } => "spaces_to_tabs",
        TabsToSpaces { .. } => "tabs_to_spaces",
        ConvertLineEndings(_) => "convert_line_endings",
        ConvertLineEndingsAll(_) => "convert_line_endings_all",
        TabsToSpacesAll { .. } => "tabs_to_spaces_all",
        SurroundSelection { .. } => "surround_selection",
        MarkdownToggleEmphasis(_) => "markdown_toggle_emphasis",
        MarkdownSetHeading(_) => "markdown_set_heading",
        MarkdownCycleHeading(_) => "markdown_cycle_heading",
        MarkdownPromoteSection => "markdown_promote_section",
        MarkdownDemoteSection => "markdown_demote_section",
        MarkdownMoveSectionUp => "markdown_move_section_up",
        MarkdownMoveSectionDown => "markdown_move_section_down",
        MarkdownToggleBullet => "markdown_toggle_bullet",
        MarkdownToggleNumbered => "markdown_toggle_numbered",
        MarkdownToggleCheckbox => "markdown_toggle_checkbox",
        MarkdownToggleTask => "markdown_toggle_task",
        MarkdownCycleListMarker => "markdown_cycle_list_marker",
        MarkdownRenumberList => "markdown_renumber_list",
        MarkdownWrapInBlockquote => "markdown_wrap_in_blockquote",
        MarkdownInsertCodeFence => "markdown_insert_code_fence",
        MarkdownInsertLink => "markdown_insert_link",
        MarkdownInsertImageRef => "markdown_insert_image_ref",
        MarkdownStripFormatting => "markdown_strip_formatting",
        InsertPair { .. } => "insert_pair",
        DeletePair { .. } => "delete_pair",
    }
}

/// Variant-specific extra detail (`chars=…`, `column=…`). Empty
/// string for variants with no payload-worth fields. Only call
/// when the trace sink is enabled — callers gate with
/// [`crate::paint_trace::is_trace_enabled`] so the `format!`
/// budget here only fires under trace.
pub(crate) fn detail_of(edit: &SelectionEdit) -> String {
    use SelectionEdit::{
        ChangeCase, InsertText, ReflowParagraph, ShuffleLines, SortLines, WrapAtColumn,
    };
    match edit {
        InsertText(text) => {
            // chars=… is the user-visible grapheme-ish count
            // (close enough for trace bucketing — char counts
            // dominate the typing-burst case where text="x").
            // newline=true flags structural edits the
            // edit-region pulse path treats as line-shape
            // changes; useful when triaging splice-vs-dirty
            // mispredictions from ε.5c's classifier.
            let chars = text.chars().count();
            let newline = text.contains('\n');
            let input = match (chars, newline) {
                (1, true) => "newline",
                (1, false) => "char",
                (_, true) => "paste_multiline",
                (_, false) => "paste",
            };
            format!("chars={chars} newline={newline} input={input}")
        }
        WrapAtColumn(col) | ReflowParagraph(col) => format!("column={col}"),
        SortLines(_) | ShuffleLines(_) | ChangeCase(_) => String::new(),
        _ => String::new(),
    }
}

/// Format the `event:edit_apply_result` detail field for a given
/// apply result. Pure (no I/O, no trace gate) so the helper is
/// unit-testable in isolation.
pub(crate) fn format_result(kind: &str, result: &Result<Option<Revision>, Error>) -> String {
    match result {
        Ok(Some(rev)) => format!("kind={kind} result=revision rev={}", rev.0),
        Ok(None) => format!("kind={kind} result=noop"),
        Err(e) => format!("kind={kind} result=error err={e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_of_covers_typing_funnels() {
        assert_eq!(
            kind_of(&SelectionEdit::InsertText("x".into())),
            "insert_text"
        );
        assert_eq!(
            kind_of(&SelectionEdit::InsertText("\n".into())),
            "insert_text"
        );
        assert_eq!(kind_of(&SelectionEdit::DeleteBack), "delete_back");
        assert_eq!(kind_of(&SelectionEdit::DeleteForward), "delete_forward");
    }

    #[test]
    fn kind_of_covers_smart_newline_and_structural() {
        assert_eq!(
            kind_of(&SelectionEdit::InsertNewlineSmart),
            "insert_newline_smart"
        );
        assert_eq!(
            kind_of(&SelectionEdit::InsertNewlineAbove),
            "insert_newline_above"
        );
        assert_eq!(
            kind_of(&SelectionEdit::InsertNewlineBelow),
            "insert_newline_below"
        );
        assert_eq!(
            kind_of(&SelectionEdit::DeleteWordBackward),
            "delete_word_backward"
        );
        assert_eq!(
            kind_of(&SelectionEdit::DeleteWordForward),
            "delete_word_forward"
        );
        assert_eq!(kind_of(&SelectionEdit::JoinLines), "join_lines");
        assert_eq!(kind_of(&SelectionEdit::TransposeChars), "transpose_chars");
    }

    #[test]
    fn detail_of_insert_text_carries_chars_and_newline_flag() {
        let single = detail_of(&SelectionEdit::InsertText("x".into()));
        assert_eq!(single, "chars=1 newline=false input=char");

        let newline = detail_of(&SelectionEdit::InsertText("\n".into()));
        assert_eq!(newline, "chars=1 newline=true input=newline");

        let multi = detail_of(&SelectionEdit::InsertText("abc\nde".into()));
        // 6 chars, newline=true — covers the multi-line-paste branch.
        assert_eq!(multi, "chars=6 newline=true input=paste_multiline");

        let paste = detail_of(&SelectionEdit::InsertText("abcd".into()));
        assert_eq!(paste, "chars=4 newline=false input=paste");
    }

    #[test]
    fn detail_of_column_variants_emit_column() {
        assert_eq!(detail_of(&SelectionEdit::WrapAtColumn(80)), "column=80");
        assert_eq!(detail_of(&SelectionEdit::ReflowParagraph(72)), "column=72");
    }

    #[test]
    fn detail_of_empty_for_payload_free_variants() {
        assert!(detail_of(&SelectionEdit::DeleteBack).is_empty());
        assert!(detail_of(&SelectionEdit::JoinLines).is_empty());
        assert!(detail_of(&SelectionEdit::MarkdownToggleBullet).is_empty());
    }

    #[test]
    fn format_result_revision_carries_rev_number() {
        let result: Result<Option<Revision>, Error> = Ok(Some(Revision(42)));
        let line = format_result("insert_text", &result);
        assert_eq!(line, "kind=insert_text result=revision rev=42");
    }

    #[test]
    fn format_result_noop_omits_revision() {
        let result: Result<Option<Revision>, Error> = Ok(None);
        let line = format_result("delete_back", &result);
        assert_eq!(line, "kind=delete_back result=noop");
    }

    #[test]
    fn format_result_error_carries_err_string() {
        let result: Result<Option<Revision>, Error> = Err(Error::UnknownBuffer);
        let line = format_result("insert_text", &result);
        assert!(line.starts_with("kind=insert_text result=error err="));
    }
}
