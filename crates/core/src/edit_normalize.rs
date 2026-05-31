//! Whole-buffer normalize planners — used by the Phase C2 status-bar
//! click-to-act handler and the Phase C3 mixed-LE / mixed-indent
//! warning chips.
//!
//! These siblings of [`crate::edit_line_text`] re-use the same
//! [`crate::edit_planning::EditSpec`] / [`crate::edit_planning::finalize_specs`] machinery but
//! ignore selection bounds — every line in the buffer is rewritten.
//! Each plan rides one undo group via the standard
//! `apply_selection_edit` path.

use continuity_buffer::Buffer;

use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;
use crate::LineEnding;

/// Phase C2 — convert line endings on every line in the buffer,
/// regardless of selection. Used by the status-bar click handler and
/// the C3 mixed-LE chip.
pub(crate) fn plan_convert_line_endings_all(
    buffer: &Buffer,
    eol: LineEnding,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let target = match eol {
        LineEnding::Lf => "\n",
        LineEnding::Crlf => "\r\n",
    };
    let mut specs = Vec::new();
    for line in 0..rope.len_lines() {
        if line + 1 >= rope.len_lines() {
            continue;
        }
        let content_end = line_content_end(rope, line);
        let next_start = rope.line_to_byte(line + 1);
        if next_start <= content_end {
            continue;
        }
        let current = rope.byte_slice(content_end..next_start).to_string();
        if current != target {
            specs.push(EditSpec::replace(
                rope,
                content_end,
                next_start,
                target.to_string(),
            )?);
        }
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

/// Phase C3 — replace every tab in the buffer with `tab_width` spaces,
/// regardless of selection.
pub(crate) fn plan_tabs_to_spaces_all(
    buffer: &Buffer,
    tab_width: u32,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let spaces: String = " ".repeat(tab_width as usize);
    let mut specs = Vec::new();
    let total_lines = rope.len_lines();
    for line in 0..total_lines {
        let start = rope.line_to_byte(line);
        let end = line_content_end(rope, line);
        let slice = rope.byte_slice(start..end).to_string();
        if !slice.contains('\t') {
            continue;
        }
        let replaced = slice.replace('\t', &spaces);
        specs.push(EditSpec::replace(rope, start, end, replaced)?);
    }
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

#[cfg(test)]
mod tests {
    use crate::selection_edit::{apply_plan, plan, SelectionEdit};
    use crate::LineEnding;
    use continuity_buffer::Buffer;
    use continuity_text::{Position, Selection};

    fn apply_via_plan(buffer: &mut Buffer, edit: &SelectionEdit) {
        if let Some(p) = plan(buffer, edit).expect("plan ok") {
            apply_plan(buffer, &p).expect("apply ok");
        }
    }

    #[test]
    fn convert_line_endings_all_normalises_mixed_to_lf() {
        // Phase C2 — chip click + status-bar LE toggle dispatch this.
        // Caret-only selection covers a single line; the `*_all` plan
        // must still rewrite every line in the buffer.
        let mut b = Buffer::from_text("a\nb\r\nc\n");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        apply_via_plan(
            &mut b,
            &SelectionEdit::ConvertLineEndingsAll(LineEnding::Lf),
        );
        assert_eq!(b.rope().to_string(), "a\nb\nc\n");
    }

    #[test]
    fn convert_line_endings_all_lf_to_crlf_whole_buffer() {
        let mut b = Buffer::from_text("a\nb\nc\n");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        apply_via_plan(
            &mut b,
            &SelectionEdit::ConvertLineEndingsAll(LineEnding::Crlf),
        );
        assert_eq!(b.rope().to_string(), "a\r\nb\r\nc\r\n");
    }

    #[test]
    fn tabs_to_spaces_all_replaces_every_tab() {
        // Phase C3 chip click — buffer has mixed indent; whole-buffer
        // normalize must convert tabs in every line, not just covered.
        let mut b = Buffer::from_text("    a\n\tb\n");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        apply_via_plan(&mut b, &SelectionEdit::TabsToSpacesAll { tab_width: 4 });
        assert_eq!(b.rope().to_string(), "    a\n    b\n");
    }

    #[test]
    fn tabs_to_spaces_all_noop_when_no_tabs() {
        // Planner returns Ok(None) when no edit is necessary.
        let mut b = Buffer::from_text("    a\n    b\n");
        b.set_selections(vec![Selection::caret_at(Position::new(0, 0))]);
        let p = plan(&b, &SelectionEdit::TabsToSpacesAll { tab_width: 4 }).expect("plan ok");
        assert!(p.is_none(), "no tabs in buffer → no plan");
    }
}
