//! Context-free command names shared by storage-neutral hosts.

use continuity_engine::{CaseKind, EmphasisKind, IndentUnit, LineEnding, SelectionEdit, SortKind};

use crate::EditorOperation;

/// Resolve a context-free editor command to its typed engine operation.
///
/// `None` means the command requires host mediation or is unknown.
#[must_use]
pub fn editor_operation_for_command(command: &str) -> Option<EditorOperation> {
    let edit = match command {
        "editor.indent" => SelectionEdit::Indent {
            unit: IndentUnit::Tab,
        },
        "editor.outdent" => SelectionEdit::Outdent {
            unit: IndentUnit::Tab,
        },
        "editor.insert_newline_above" => SelectionEdit::InsertNewlineAbove,
        "editor.insert_newline_below" => SelectionEdit::InsertNewlineBelow,
        "editor.insert_newline_smart" => SelectionEdit::InsertNewlineSmart,
        "editor.delete_word_backward" => SelectionEdit::DeleteWordBackward,
        "editor.delete_word_forward" => SelectionEdit::DeleteWordForward,
        "editor.delete_to_line_start" => SelectionEdit::DeleteToLineStart,
        "editor.delete_to_line_end" => SelectionEdit::DeleteToLineEnd,
        "editor.delete_to_bracket" => SelectionEdit::DeleteToBracket,
        "editor.duplicate_line" => SelectionEdit::DuplicateLine,
        "editor.duplicate_selection" => SelectionEdit::DuplicateSelection,
        "editor.move_line_up_block" => SelectionEdit::MoveLineUp,
        "editor.move_line_down_block" => SelectionEdit::MoveLineDown,
        "editor.join_lines" => SelectionEdit::JoinLines,
        "editor.join_selected_lines" => SelectionEdit::JoinSelectedLines,
        "editor.toggle_bullet_at_line_start" => SelectionEdit::ToggleBulletAtLineStart,
        "editor.toggle_bullet_indent_continuation" => {
            SelectionEdit::ToggleBulletWithContinuationIndent {
                unit: IndentUnit::Tab,
            }
        }
        "editor.sort_lines_asc" => SelectionEdit::SortLines(SortKind::Asc),
        "editor.sort_lines_desc" => SelectionEdit::SortLines(SortKind::Desc),
        "editor.sort_lines_asc_ci" => SelectionEdit::SortLines(SortKind::AscCaseInsensitive),
        "editor.sort_lines_desc_ci" => SelectionEdit::SortLines(SortKind::DescCaseInsensitive),
        "editor.sort_lines_asc_len" => SelectionEdit::SortLines(SortKind::AscByLength),
        "editor.sort_lines_desc_len" => SelectionEdit::SortLines(SortKind::DescByLength),
        "editor.sort_lines_asc_num" => SelectionEdit::SortLines(SortKind::AscNumeric),
        "editor.sort_lines_desc_num" => SelectionEdit::SortLines(SortKind::DescNumeric),
        "editor.reverse_lines" => SelectionEdit::ReverseLines,
        "editor.unique_lines" => SelectionEdit::UniqueLines,
        "editor.shuffle_lines" => SelectionEdit::ShuffleLines(0xCAFE_F00D),
        "editor.trim_trailing_whitespace" => SelectionEdit::TrimTrailingWhitespaceAll,
        "editor.trim_whitespace" => SelectionEdit::TrimWhitespaceAll,
        "editor.wrap_at_column" => SelectionEdit::WrapAtColumn(80),
        "editor.reflow_paragraph" => SelectionEdit::ReflowParagraph(80),
        "editor.transpose_chars" => SelectionEdit::TransposeChars,
        "editor.transpose_words" => SelectionEdit::TransposeWords,
        "editor.change_case_upper" => SelectionEdit::ChangeCase(CaseKind::Upper),
        "editor.change_case_lower" => SelectionEdit::ChangeCase(CaseKind::Lower),
        "editor.change_case_title" => SelectionEdit::ChangeCase(CaseKind::Title),
        "editor.change_case_toggle" => SelectionEdit::ChangeCase(CaseKind::Toggle),
        "editor.change_case_sentence" => SelectionEdit::ChangeCase(CaseKind::Sentence),
        "editor.convert_line_endings_lf" => SelectionEdit::ConvertLineEndings(LineEnding::Lf),
        "editor.convert_line_endings_crlf" => SelectionEdit::ConvertLineEndings(LineEnding::Crlf),
        "editor.surround_parens" => surround("(", ")"),
        "editor.surround_brackets" => surround("[", "]"),
        "editor.surround_braces" => surround("{", "}"),
        "editor.surround_double_quotes" => surround("\"", "\""),
        "markdown.toggle_bold" => emphasis(EmphasisKind::Bold),
        "markdown.toggle_italic" => emphasis(EmphasisKind::Italic),
        "markdown.toggle_strikethrough" => emphasis(EmphasisKind::Strikethrough),
        "markdown.toggle_inline_code" => emphasis(EmphasisKind::InlineCode),
        "markdown.set_heading_1" => SelectionEdit::MarkdownSetHeading(1),
        "markdown.set_heading_2" => SelectionEdit::MarkdownSetHeading(2),
        "markdown.set_heading_3" => SelectionEdit::MarkdownSetHeading(3),
        "markdown.set_heading_4" => SelectionEdit::MarkdownSetHeading(4),
        "markdown.set_heading_5" => SelectionEdit::MarkdownSetHeading(5),
        "markdown.set_heading_6" => SelectionEdit::MarkdownSetHeading(6),
        "markdown.remove_heading" => SelectionEdit::MarkdownSetHeading(0),
        "markdown.cycle_heading_up" => SelectionEdit::MarkdownCycleHeading(1),
        "markdown.cycle_heading_down" => SelectionEdit::MarkdownCycleHeading(-1),
        "markdown.promote_section" => SelectionEdit::MarkdownPromoteSection,
        "markdown.demote_section" => SelectionEdit::MarkdownDemoteSection,
        "markdown.move_section_up" => SelectionEdit::MarkdownMoveSectionUp,
        "markdown.move_section_down" => SelectionEdit::MarkdownMoveSectionDown,
        "markdown.toggle_bullet" => SelectionEdit::MarkdownToggleBullet,
        "markdown.toggle_numbered" => SelectionEdit::MarkdownToggleNumbered,
        "markdown.toggle_checkbox" => SelectionEdit::MarkdownToggleCheckbox,
        "markdown.toggle_task" => SelectionEdit::MarkdownToggleTask,
        "markdown.strip_formatting" => SelectionEdit::MarkdownStripFormatting,
        "markdown.cycle_list_marker" => SelectionEdit::MarkdownCycleListMarker,
        "markdown.renumber_list" => SelectionEdit::MarkdownRenumberList,
        "markdown.wrap_in_blockquote" => SelectionEdit::MarkdownWrapInBlockquote,
        "markdown.insert_code_fence" => SelectionEdit::MarkdownInsertCodeFence,
        "markdown.insert_link" => SelectionEdit::MarkdownInsertLink,
        "markdown.insert_image_ref" => SelectionEdit::MarkdownInsertImageRef,
        _ => return operation_without_selection_edit(command),
    };
    Some(EditorOperation::ApplySelectionEdit(edit))
}

fn operation_without_selection_edit(command: &str) -> Option<EditorOperation> {
    match command {
        "editor.select_word" => Some(EditorOperation::SelectWord),
        "editor.select_line" => Some(EditorOperation::SelectLine),
        "editor.undo" => Some(EditorOperation::Undo),
        "editor.redo" => Some(EditorOperation::Redo),
        "editor.redo_alternate_branch" => Some(EditorOperation::RedoAlternate),
        _ => None,
    }
}

fn surround(open: &str, close: &str) -> SelectionEdit {
    SelectionEdit::SurroundSelection {
        open: open.into(),
        close: close.into(),
    }
}

fn emphasis(kind: EmphasisKind) -> SelectionEdit {
    SelectionEdit::MarkdownToggleEmphasis(kind)
}

#[cfg(test)]
mod tests {
    use super::editor_operation_for_command;
    use crate::EditorOperation;
    use continuity_engine::{EmphasisKind, SelectionEdit};

    #[test]
    fn reusable_commands_resolve_to_typed_operations() {
        assert!(matches!(
            editor_operation_for_command("editor.join_lines"),
            Some(EditorOperation::ApplySelectionEdit(
                SelectionEdit::JoinLines
            ))
        ));
        assert!(matches!(
            editor_operation_for_command("markdown.toggle_bold"),
            Some(EditorOperation::ApplySelectionEdit(
                SelectionEdit::MarkdownToggleEmphasis(EmphasisKind::Bold)
            ))
        ));
        assert!(matches!(
            editor_operation_for_command("editor.undo"),
            Some(EditorOperation::Undo)
        ));
        assert!(matches!(
            editor_operation_for_command("editor.redo"),
            Some(EditorOperation::Redo)
        ));
        assert!(matches!(
            editor_operation_for_command("editor.redo_alternate_branch"),
            Some(EditorOperation::RedoAlternate)
        ));
        assert!(matches!(
            editor_operation_for_command("editor.select_word"),
            Some(EditorOperation::SelectWord)
        ));
        assert!(matches!(
            editor_operation_for_command("editor.select_line"),
            Some(EditorOperation::SelectLine)
        ));
    }

    #[test]
    fn host_context_commands_do_not_resolve() {
        for command in [
            "file.open",
            "pane.split_vertical",
            "tab.close",
            "window.new_window",
            "settings.open",
            "editor.insert_char",
        ] {
            assert!(editor_operation_for_command(command).is_none(), "{command}");
        }
    }
}
