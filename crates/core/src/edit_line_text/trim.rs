//! Whitespace-trimming line-text planners.
//!
//! Split out of `edit_line_text.rs` so that file stays under the 600-line
//! cap. Three entry points:
//! - [`plan_trim_trailing_whitespace`] — trailing-only, selection-scoped.
//! - [`plan_trim_trailing_whitespace_all`] — trailing-only, whole buffer.
//! - [`plan_trim_whitespace_all`] — leading **and** trailing, whole buffer.
//!
//! All emit per-line delete specs and let [`finalize_specs`] sort them
//! descending into a single undo group.

use continuity_buffer::Buffer;

use crate::edit_lines::lines_covered;
use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

pub(crate) fn plan_trim_trailing_whitespace(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let lines = lines_covered(buffer);
    plan_trim_lines(buffer, &lines)
}

/// Phase B14 — trim trailing whitespace on every line in the buffer.
pub(crate) fn plan_trim_trailing_whitespace_all(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let total = buffer.rope().len_lines();
    let lines: Vec<usize> = (0..total).collect();
    plan_trim_lines(buffer, &lines)
}

fn plan_trim_lines(buffer: &Buffer, lines: &[usize]) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    for &line in lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let slice = rope.byte_slice(start..end).to_string();
        let trimmed = slice.trim_end_matches([' ', '\t']);
        if trimmed.len() < slice.len() {
            specs.push(EditSpec::delete(rope, start + trimmed.len(), end)?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

/// Strip leading **and** trailing whitespace from every line in the buffer
/// (one undo group, fully undoable). Used by the explicit
/// `editor.trim_whitespace` command.
///
/// Per-line, two byte ranges may be deleted: the leading whitespace run
/// (`line_start .. line_start + leading_len`) and the trailing whitespace
/// run (`line_start + trimmed_end .. line_end`). Both specs are queued and
/// [`finalize_specs`] sorts them descending into a single group; an
/// all-whitespace line's two overlapping deletes merge into one.
///
/// NOTE: per-line *leading* strip removes indentation by design — this is
/// the literal "trim each line" interpretation, distinct from
/// `editor.trim_trailing_whitespace`, which preserves indentation.
pub(crate) fn plan_trim_whitespace_all(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let total = buffer.rope().len_lines();
    let lines: Vec<usize> = (0..total).collect();
    plan_trim_leading_and_trailing(buffer, &lines)
}

fn plan_trim_leading_and_trailing(
    buffer: &Buffer,
    lines: &[usize],
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    for &line in lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let slice = rope.byte_slice(start..end).to_string();
        // Trailing run.
        let trimmed_end = slice.trim_end_matches([' ', '\t']);
        if trimmed_end.len() < slice.len() {
            specs.push(EditSpec::delete(rope, start + trimmed_end.len(), end)?);
        }
        // Leading run, measured on the original slice so the two deletes are
        // computed against the same pre-edit offsets.
        let leading_len = slice.len() - slice.trim_start_matches([' ', '\t']).len();
        if leading_len > 0 {
            specs.push(EditSpec::delete(rope, start, start + leading_len)?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    use crate::selection_edit::{apply_plan, plan, SelectionEdit};

    fn build(text: &str) -> Buffer {
        let mut b = Buffer::from_text(text);
        b.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        b
    }

    fn run(b: &mut Buffer, edit: SelectionEdit) {
        let p = plan(b, &edit).expect("plan ok").expect("plan some");
        apply_plan(b, &p).expect("apply ok");
    }

    #[test]
    fn trim_whitespace_all_strips_leading_and_trailing() {
        let mut b = build("  foo  \n\tbar\t\n   baz");
        run(&mut b, SelectionEdit::TrimWhitespaceAll);
        assert_eq!(b.rope().to_string(), "foo\nbar\nbaz");
    }

    #[test]
    fn trim_whitespace_all_collapses_whitespace_only_lines() {
        let mut b = build("a\n   \nb");
        run(&mut b, SelectionEdit::TrimWhitespaceAll);
        assert_eq!(b.rope().to_string(), "a\n\nb");
    }

    #[test]
    fn trim_whitespace_all_single_plan_covers_both_lines() {
        // Both affected lines are rewritten by one plan (one undo group).
        let mut b = build("  a  \n  b  ");
        let p = plan(&b, &SelectionEdit::TrimWhitespaceAll)
            .expect("plan ok")
            .expect("plan some");
        // Four deletes: leading+trailing on each of two lines.
        assert_eq!(p.ops.len(), 4);
        apply_plan(&mut b, &p).expect("apply ok");
        assert_eq!(b.rope().to_string(), "a\nb");
    }

    #[test]
    fn trim_whitespace_all_noop_on_clean_buffer() {
        let b = build("foo\nbar");
        let p = plan(&b, &SelectionEdit::TrimWhitespaceAll).expect("plan ok");
        assert!(p.is_none(), "clean buffer must produce no plan");
    }

    #[test]
    fn trim_trailing_only_preserves_indentation() {
        // Sanity: trailing-only keeps leading whitespace (the contrast with
        // TrimWhitespaceAll).
        let mut b = build("   foo   ");
        run(&mut b, SelectionEdit::TrimTrailingWhitespaceAll);
        assert_eq!(b.rope().to_string(), "   foo");
    }
}
