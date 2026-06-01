//! Selection-aware edit planning for the core thread.
//!
//! Every command that mutates buffer text routes through here. The public
//! [`SelectionEdit`] enum names the operation; [`plan`] turns it into a
//! [`SelectionEditPlan`] of atomic [`EditOp`]s in **descending byte order**
//! (so sequential `Buffer::apply` calls keep their pre-edit offsets valid)
//! plus the post-edit selection set; [`apply_plan`] runs the ops against a
//! mutable buffer and returns the final revision.
//!
//! Per-family planners (newlines, word-scope deletes, line ops, inline
//! shape edits, markdown) live in sibling modules to keep this file under
//! the 600-line cap.

use continuity_buffer::{Buffer, Revision};
use continuity_text::{EditOp, Selection};
use ropey::Rope;

use crate::edit_inline::{
    plan_change_case, plan_delete_to_bracket, plan_reflow_paragraph, plan_surround_selection,
    plan_transpose_chars, plan_wrap_at_column,
};
use crate::edit_line_text::{
    plan_convert_line_endings, plan_indent, plan_outdent, plan_reverse_lines, plan_shuffle_lines,
    plan_sort_lines, plan_spaces_to_tabs, plan_tabs_to_spaces, plan_trim_trailing_whitespace,
    plan_trim_trailing_whitespace_all, plan_unique_lines,
};
use crate::edit_lines::{
    plan_delete_to_line_end, plan_delete_to_line_start, plan_duplicate_line,
    plan_duplicate_selection, plan_insert_newline_above, plan_insert_newline_below,
    plan_insert_newline_smart, plan_join_lines, plan_move_line_down, plan_move_line_up,
    plan_toggle_bullet_at_line_start,
};
use crate::edit_markdown::{
    plan_markdown_cycle_list_marker, plan_markdown_insert_code_fence,
    plan_markdown_insert_image_ref, plan_markdown_insert_link, plan_markdown_toggle_bullet,
    plan_markdown_toggle_checkbox, plan_markdown_toggle_emphasis, plan_markdown_toggle_numbered,
    plan_markdown_toggle_task, plan_markdown_wrap_in_blockquote,
};
use crate::edit_markdown_blocks::{
    plan_markdown_cycle_heading, plan_markdown_demote_section, plan_markdown_move_section_down,
    plan_markdown_move_section_up, plan_markdown_promote_section, plan_markdown_set_heading,
};
use crate::edit_pairs::{plan_delete_pair, plan_insert_pair};
use crate::edit_planning::{
    advance_position, caret_delete_range, merge_specs, ranges_for_selection, EditSpec,
};
use crate::edit_words::{
    plan_delete_word_backward, plan_delete_word_forward, plan_transpose_words,
};
use crate::Error;

/// A selection-aware edit requested by an editor command.
///
/// New variants land here as commands graduate from "named in spec" to
/// "implemented". Each variant maps to exactly one undo group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionEdit {
    /// Insert or replace each active selection with the supplied text.
    InsertText(String),
    /// Delete the selected text, or one character before each caret.
    DeleteBack,
    /// Delete the selected text, or one character after each caret.
    DeleteForward,

    /// Insert a newline on a fresh line above each selection's line, with
    /// the caret landing on the new line.
    InsertNewlineAbove,
    /// Insert a newline on a fresh line below each selection's line, caret
    /// at the new line.
    InsertNewlineBelow,
    /// Insert a newline that inherits the leading indentation of the
    /// caret's current line.
    InsertNewlineSmart,

    /// Toggle a `- ` bullet marker right after each selection line's
    /// leading whitespace. Adding the marker shifts the caret's byte
    /// column by `+2` so the cursor stays on the same content character
    /// (visually unchanged). Removing it shifts by `-2` (clamped to the
    /// leading-whitespace column). Plain-text variant of
    /// [`SelectionEdit::MarkdownToggleBullet`] — it doesn't depend on
    /// markdown parse state and is meant for the Ctrl+R quick-toggle
    /// shortcut.
    ToggleBulletAtLineStart,

    /// Delete text from the caret backward to the previous word boundary.
    DeleteWordBackward,
    /// Delete text from the caret forward to the next word boundary.
    DeleteWordForward,
    /// Delete from the caret back to the start of the line (or first
    /// non-whitespace if already at line start).
    DeleteToLineStart,
    /// Delete from the caret forward to the end of the line.
    DeleteToLineEnd,
    /// Delete from the caret to the matching bracket, if any.
    DeleteToBracket,

    /// Duplicate the entire line(s) covered by each selection.
    DuplicateLine,
    /// Duplicate the bytes spanned by each selection (caret selections
    /// duplicate nothing).
    DuplicateSelection,
    /// Move each selected line up one position.
    MoveLineUp,
    /// Move each selected line down one position.
    MoveLineDown,
    /// Join the line below each selection's line into the current line.
    JoinLines,
    /// Sort the lines covered by selections.
    SortLines(SortKind),
    /// Reverse the order of lines covered by selections.
    ReverseLines,
    /// Drop duplicate lines covered by selections (case-sensitive, stable).
    UniqueLines,
    /// Pseudo-randomly reorder the lines covered by selections, seeded for
    /// determinism (the seed is hashed into the LCG).
    ShuffleLines(u64),
    /// Trim trailing whitespace from every line covered by selections.
    TrimTrailingWhitespace,
    /// Phase B14: trim trailing whitespace on every line in the buffer
    /// regardless of selection. Used by the explicit
    /// `editor.trim_trailing_whitespace` command and the on-save hook.
    TrimTrailingWhitespaceAll,

    /// Hard-wrap each paragraph covered by selections at `column` columns.
    WrapAtColumn(u32),
    /// Reflow each paragraph at `column` columns, preserving leading
    /// whitespace.
    ReflowParagraph(u32),

    /// Transpose the two characters straddling each caret.
    TransposeChars,
    /// Transpose the two words straddling each caret.
    TransposeWords,

    /// Change case of the bytes inside each selection (or the word at each
    /// caret).
    ChangeCase(CaseKind),

    /// Indent each selected line by one indent unit.
    Indent {
        /// Indent unit (tab or N spaces).
        unit: IndentUnit,
    },
    /// Outdent each selected line by one indent unit.
    Outdent {
        /// Indent unit (tab or N spaces).
        unit: IndentUnit,
    },

    /// Replace runs of `tab_width` spaces with tabs.
    SpacesToTabs {
        /// Number of contiguous spaces that map to one tab.
        tab_width: u32,
    },
    /// Replace tabs with `tab_width` spaces.
    TabsToSpaces {
        /// Number of spaces emitted per tab.
        tab_width: u32,
    },
    /// Convert line endings of every covered line.
    ConvertLineEndings(LineEnding),
    /// Phase C2 — convert line endings on every line in the buffer
    /// regardless of selection. Used by the status-bar click handler
    /// and the C3 mixed-LE normalize chip.
    ConvertLineEndingsAll(LineEnding),
    /// Phase C3 — replace every tab in the buffer with `tab_width`
    /// spaces, regardless of selection. Used by the mixed-indent
    /// normalize chip.
    TabsToSpacesAll {
        /// Number of spaces emitted per tab.
        tab_width: u32,
    },

    /// Wrap each non-empty selection in `open` … `close`.
    SurroundSelection {
        /// Inserted at the start of the selection.
        open: String,
        /// Inserted at the end of the selection.
        close: String,
    },

    /// Toggle markdown emphasis around each selection.
    MarkdownToggleEmphasis(EmphasisKind),
    /// Set the heading level of each covered line. `level == 0` strips the
    /// heading; `1..=6` rewrites the prefix.
    MarkdownSetHeading(u8),
    /// Cycle the heading level on each covered line (`+1`/`-1`).
    MarkdownCycleHeading(i32),
    /// Promote (decrease level) the section enclosing each caret.
    MarkdownPromoteSection,
    /// Demote (increase level) the section enclosing each caret.
    MarkdownDemoteSection,
    /// Move the section enclosing each caret upward past the previous
    /// sibling section.
    MarkdownMoveSectionUp,
    /// Move the section enclosing each caret downward past the next
    /// sibling section.
    MarkdownMoveSectionDown,
    /// Toggle a `- ` bullet prefix on each covered line.
    MarkdownToggleBullet,
    /// Toggle a `1. ` numbered prefix on each covered line.
    MarkdownToggleNumbered,
    /// Toggle a `[ ]`/`[x]` checkbox prefix on each covered line.
    MarkdownToggleCheckbox,
    /// Toggle a `- [ ] ` task-bullet prefix on each covered line: plain
    /// or bulleted lines gain the task marker; existing task lines drop
    /// it. Bound to `Ctrl+E`.
    MarkdownToggleTask,
    /// Cycle the list marker between `-`, `*`, `+`.
    MarkdownCycleListMarker,
    /// Phase B11: renumber the ordered list containing the caret.
    MarkdownRenumberList,
    /// Prefix `> ` on each covered line.
    MarkdownWrapInBlockquote,
    /// Wrap selections in fenced code blocks (or insert an empty fence).
    MarkdownInsertCodeFence,
    /// Wrap each non-empty selection as `[text](url)` with placeholder url.
    MarkdownInsertLink,
    /// Insert `![alt](path)` at each caret.
    MarkdownInsertImageRef,

    /// Phase-16.5 auto-pair insert: at each caret, insert `open`
    /// immediately followed by `close`, leaving the caret between
    /// them. Lands as one undo group. Selections that already cover a
    /// non-empty range fall back to surrounding the range with
    /// `open` … `close` (so highlight + `(` keeps the surround
    /// behavior of [`Self::SurroundSelection`]).
    InsertPair {
        /// Leading character (e.g. `(`).
        open: String,
        /// Trailing character (e.g. `)`).
        close: String,
    },
    /// Phase-16.5 backspace-aware delete-pair: at each caret, if the
    /// preceding byte is `open` and the following byte is `close`,
    /// delete both as one undo group. Otherwise the planner returns
    /// `Ok(None)` so the caller can fall through to a normal
    /// [`Self::DeleteBack`].
    DeletePair {
        /// Open delimiter to the left of the caret.
        open: char,
        /// Close delimiter to the right of the caret.
        close: char,
    },
}

/// Sort variant used by [`SelectionEdit::SortLines`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SortKind {
    /// Ascending, case-sensitive byte order.
    Asc,
    /// Descending, case-sensitive byte order.
    Desc,
    /// Ascending, case-insensitive.
    AscCaseInsensitive,
    /// Descending, case-insensitive.
    DescCaseInsensitive,
    /// Ascending, by ASCII length.
    AscByLength,
    /// Descending, by ASCII length.
    DescByLength,
    /// Ascending, natural-numeric (digit runs as integers).
    AscNumeric,
    /// Descending, natural-numeric.
    DescNumeric,
}

/// Case-conversion kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CaseKind {
    /// `EXAMPLE`.
    Upper,
    /// `example`.
    Lower,
    /// `Example Title`.
    Title,
    /// Invert the case of each ASCII letter.
    Toggle,
    /// `Example. case.` — first letter of each sentence uppercased.
    Sentence,
}

/// Indent unit used by [`SelectionEdit::Indent`] / [`SelectionEdit::Outdent`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IndentUnit {
    /// One tab character.
    Tab,
    /// `n` spaces.
    Spaces(u32),
}

/// Line-ending convention.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineEnding {
    /// `\n`.
    Lf,
    /// `\r\n`.
    Crlf,
}

/// Markdown emphasis flavor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EmphasisKind {
    /// `**text**`.
    Bold,
    /// `*text*`.
    Italic,
    /// `~~text~~`.
    Strikethrough,
    /// `` `text` ``.
    InlineCode,
}

/// A planned selection edit against one immutable pre-edit buffer state.
pub struct SelectionEditPlan {
    /// Atomic edit ops to apply, ordered from highest byte offset to lowest.
    pub ops: Vec<EditOp>,
    /// Selection set before the edit.
    pub selections_before: Vec<Selection>,
    /// Selection set after the whole edit group.
    pub selections_after: Vec<Selection>,
}

/// Build atomic edit ops for a selection-aware edit.
///
/// Returns `Ok(None)` when the operation has no effect (e.g. backspace at
/// the document start with all carets at offset 0).
///
/// # Errors
///
/// Returns core errors when any selection position is outside the buffer,
/// or argument validation fails (e.g. an out-of-range heading level).
pub fn plan(buffer: &Buffer, edit: &SelectionEdit) -> Result<Option<SelectionEditPlan>, Error> {
    match edit {
        SelectionEdit::InsertText(text) => plan_insert_text(buffer, text),
        SelectionEdit::DeleteBack => plan_delete(buffer, true),
        SelectionEdit::DeleteForward => plan_delete(buffer, false),

        SelectionEdit::InsertNewlineAbove => plan_insert_newline_above(buffer),
        SelectionEdit::InsertNewlineBelow => plan_insert_newline_below(buffer),
        SelectionEdit::InsertNewlineSmart => plan_insert_newline_smart(buffer),
        SelectionEdit::ToggleBulletAtLineStart => plan_toggle_bullet_at_line_start(buffer),

        SelectionEdit::DeleteWordBackward => plan_delete_word_backward(buffer),
        SelectionEdit::DeleteWordForward => plan_delete_word_forward(buffer),
        SelectionEdit::DeleteToLineStart => plan_delete_to_line_start(buffer),
        SelectionEdit::DeleteToLineEnd => plan_delete_to_line_end(buffer),
        SelectionEdit::DeleteToBracket => plan_delete_to_bracket(buffer),

        SelectionEdit::DuplicateLine => plan_duplicate_line(buffer),
        SelectionEdit::DuplicateSelection => plan_duplicate_selection(buffer),
        SelectionEdit::MoveLineUp => plan_move_line_up(buffer),
        SelectionEdit::MoveLineDown => plan_move_line_down(buffer),
        SelectionEdit::JoinLines => plan_join_lines(buffer),
        SelectionEdit::SortLines(kind) => plan_sort_lines(buffer, *kind),
        SelectionEdit::ReverseLines => plan_reverse_lines(buffer),
        SelectionEdit::UniqueLines => plan_unique_lines(buffer),
        SelectionEdit::ShuffleLines(seed) => plan_shuffle_lines(buffer, *seed),
        SelectionEdit::TrimTrailingWhitespace => plan_trim_trailing_whitespace(buffer),
        SelectionEdit::TrimTrailingWhitespaceAll => plan_trim_trailing_whitespace_all(buffer),

        SelectionEdit::WrapAtColumn(width) => plan_wrap_at_column(buffer, *width),
        SelectionEdit::ReflowParagraph(width) => plan_reflow_paragraph(buffer, *width),

        SelectionEdit::TransposeChars => plan_transpose_chars(buffer),
        SelectionEdit::TransposeWords => plan_transpose_words(buffer),

        SelectionEdit::ChangeCase(kind) => plan_change_case(buffer, *kind),

        SelectionEdit::Indent { unit } => plan_indent(buffer, *unit),
        SelectionEdit::Outdent { unit } => plan_outdent(buffer, *unit),

        SelectionEdit::SpacesToTabs { tab_width } => plan_spaces_to_tabs(buffer, *tab_width),
        SelectionEdit::TabsToSpaces { tab_width } => plan_tabs_to_spaces(buffer, *tab_width),
        SelectionEdit::ConvertLineEndings(eol) => plan_convert_line_endings(buffer, *eol),
        SelectionEdit::ConvertLineEndingsAll(eol) => {
            crate::edit_normalize::plan_convert_line_endings_all(buffer, *eol)
        }
        SelectionEdit::TabsToSpacesAll { tab_width } => {
            crate::edit_normalize::plan_tabs_to_spaces_all(buffer, *tab_width)
        }

        SelectionEdit::SurroundSelection { open, close } => {
            plan_surround_selection(buffer, open, close)
        }

        SelectionEdit::MarkdownToggleEmphasis(kind) => plan_markdown_toggle_emphasis(buffer, *kind),
        SelectionEdit::MarkdownSetHeading(level) => plan_markdown_set_heading(buffer, *level),
        SelectionEdit::MarkdownCycleHeading(delta) => plan_markdown_cycle_heading(buffer, *delta),
        SelectionEdit::MarkdownPromoteSection => plan_markdown_promote_section(buffer),
        SelectionEdit::MarkdownDemoteSection => plan_markdown_demote_section(buffer),
        SelectionEdit::MarkdownMoveSectionUp => plan_markdown_move_section_up(buffer),
        SelectionEdit::MarkdownMoveSectionDown => plan_markdown_move_section_down(buffer),
        SelectionEdit::MarkdownToggleBullet => plan_markdown_toggle_bullet(buffer),
        SelectionEdit::MarkdownToggleNumbered => plan_markdown_toggle_numbered(buffer),
        SelectionEdit::MarkdownToggleCheckbox => plan_markdown_toggle_checkbox(buffer),
        SelectionEdit::MarkdownToggleTask => plan_markdown_toggle_task(buffer),
        SelectionEdit::MarkdownCycleListMarker => plan_markdown_cycle_list_marker(buffer),
        SelectionEdit::MarkdownRenumberList => {
            crate::edit_list::plan_markdown_renumber_list(buffer)
        }
        SelectionEdit::MarkdownWrapInBlockquote => plan_markdown_wrap_in_blockquote(buffer),
        SelectionEdit::MarkdownInsertCodeFence => plan_markdown_insert_code_fence(buffer),
        SelectionEdit::MarkdownInsertLink => plan_markdown_insert_link(buffer),
        SelectionEdit::MarkdownInsertImageRef => plan_markdown_insert_image_ref(buffer),

        SelectionEdit::InsertPair { open, close } => plan_insert_pair(buffer, open, close),
        SelectionEdit::DeletePair { open, close } => plan_delete_pair(buffer, *open, *close),
    }
}

/// Apply already-planned ops and assign the final selection set.
///
/// # Errors
///
/// Returns buffer-layer errors if an op is invalid against the current
/// buffer state.
pub fn apply_plan(
    buffer: &mut Buffer,
    plan: &SelectionEditPlan,
) -> Result<Option<Revision>, Error> {
    let mut last = None;
    for op in &plan.ops {
        last = Some(buffer.apply(op)?);
    }
    let mut selections = plan.selections_after.clone();
    crate::selection_coalesce::coalesce_selections(&mut selections);
    buffer.set_selections(selections);
    Ok(last)
}

fn plan_insert_text(buffer: &Buffer, text: &str) -> Result<Option<SelectionEditPlan>, Error> {
    let selections_before = buffer.selections().to_vec();
    let mut specs = insert_specs(buffer.rope(), &selections_before, text)?;
    if specs.is_empty() {
        return Ok(None);
    }
    specs.sort_by_key(|spec| spec.start);
    let selections_after = specs
        .iter()
        .map(|spec| {
            let head = advance_position(spec.start_position, &spec.inserted);
            Selection::caret_at(head)
        })
        .collect();
    let ops = specs.into_iter().rev().map(EditSpec::into_op).collect();
    Ok(Some(SelectionEditPlan {
        ops,
        selections_before,
        selections_after,
    }))
}

fn plan_delete(buffer: &Buffer, backward: bool) -> Result<Option<SelectionEditPlan>, Error> {
    let selections_before = buffer.selections().to_vec();
    let mut specs = delete_specs(buffer.rope(), &selections_before, backward)?;
    if specs.is_empty() {
        return Ok(None);
    }
    specs.sort_by_key(|spec| spec.start);
    specs = merge_specs(specs);
    let selections_after = specs
        .iter()
        .map(|spec| Selection::caret_at(spec.start_position))
        .collect();
    let ops = specs.into_iter().rev().map(EditSpec::into_op).collect();
    Ok(Some(SelectionEditPlan {
        ops,
        selections_before,
        selections_after,
    }))
}

fn insert_specs(rope: &Rope, selections: &[Selection], text: &str) -> Result<Vec<EditSpec>, Error> {
    let mut specs = Vec::new();
    for selection in selections {
        for range in ranges_for_selection(rope, *selection)? {
            let start = range.start.to_byte_offset(rope)?;
            let end = range.end.to_byte_offset(rope)?;
            specs.push(EditSpec {
                start,
                end,
                start_position: range.start,
                inserted: text.to_string(),
                end_position_in_rope: Some(range.end),
            });
        }
    }
    Ok(specs)
}

fn delete_specs(
    rope: &Rope,
    selections: &[Selection],
    backward: bool,
) -> Result<Vec<EditSpec>, Error> {
    let mut specs = Vec::new();
    for selection in selections {
        for range in ranges_for_selection(rope, *selection)? {
            let start = range.start.to_byte_offset(rope)?;
            let end = range.end.to_byte_offset(rope)?;
            if start != end {
                specs.push(EditSpec {
                    start,
                    end,
                    start_position: range.start,
                    inserted: String::new(),
                    end_position_in_rope: Some(range.end),
                });
                continue;
            }
            let Some((delete_start, delete_end)) = caret_delete_range(rope, start, backward) else {
                continue;
            };
            specs.push(EditSpec::delete(rope, delete_start, delete_end)?);
        }
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection, SelectionKind};

    use super::*;

    #[test]
    fn insert_plans_one_op_per_caret_descending() {
        let mut buffer = Buffer::from_text("abcd");
        buffer.set_selections(vec![
            Selection::caret_at(Position::new(0, 1)),
            Selection::caret_at(Position::new(0, 3)),
        ]);
        let plan = plan(&buffer, &SelectionEdit::InsertText("x".into()))
            .expect("plan ok")
            .expect("plan some");
        assert_eq!(plan.ops.len(), 2);
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "axbcxd");
    }

    #[test]
    fn block_delete_uses_each_touched_line() {
        let mut buffer = Buffer::from_text("abcd\nwxyz\n");
        buffer.set_selections(vec![Selection::new(
            Position::new(0, 1),
            Position::new(1, 3),
            SelectionKind::BlockWise,
        )]);
        let plan = plan(&buffer, &SelectionEdit::DeleteForward)
            .expect("plan ok")
            .expect("plan some");
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.rope().to_string(), "ad\nwz\n");
    }

    #[test]
    fn delete_back_at_doc_start_is_noop() {
        let buffer = Buffer::from_text("abc");
        let plan = plan(&buffer, &SelectionEdit::DeleteBack).expect("plan ok");
        assert!(plan.is_none());
    }

    #[test]
    fn apply_plan_coalesces_colliding_carets() {
        // Two carets at distinct positions on the same line, both insert "x".
        // The two inserts at byte 1 and byte 2 produce post-edit carets at
        // bytes 2 and 4 — distinct, so no collision. To force a collision,
        // we wire a synthetic plan with duplicate selections_after.
        let mut buffer = Buffer::from_text("abcdef");
        buffer.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        let plan = SelectionEditPlan {
            ops: Vec::new(),
            selections_before: vec![Selection::caret_at(Position::new(0, 0))],
            selections_after: vec![
                Selection::caret_at(Position::new(0, 3)),
                Selection::caret_at(Position::new(0, 3)),
            ],
        };
        apply_plan(&mut buffer, &plan).expect("apply ok");
        assert_eq!(buffer.selections().len(), 1);
        assert_eq!(buffer.selections()[0].head, Position::new(0, 3));
    }
}
