//! Line-structure edits — newline insertion, line/selection duplication,
//! line moves, and line joins.
//!
//! Operations that modify a contiguous run of lines collapse multi-cursor
//! input into one byte range so the result is a single, deterministic
//! rewrite covered by one undo group. Line-text rewriting (sort, reverse,
//! unique, shuffle, trim, indent/outdent, tabs↔spaces, line endings) lives
//! in `edit_line_text.rs` and reuses [`lines_covered`] /
//! [`rewrite_covered_lines`] exposed here.

use continuity_buffer::Buffer;
use continuity_text::{Position, Selection};
use ropey::Rope;

use crate::edit_planning::{finalize_specs, line_content_end, EditSpec};
use crate::selection_edit::SelectionEditPlan;
use crate::Error;

pub(crate) fn plan_insert_newline_above(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let line = selection.head.line as usize;
        let line_start = rope.line_to_byte(line);
        specs.push(EditSpec::insert(rope, line_start, "\n".into())?);
        selections_after.push(Selection::caret_at(Position::new(line as u32, 0)));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_insert_newline_below(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let line = selection.head.line as usize;
        let end = line_content_end(rope, line);
        specs.push(EditSpec::insert(rope, end, "\n".into())?);
        selections_after.push(Selection::caret_at(Position::new((line as u32) + 1, 0)));
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_insert_newline_smart(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    crate::edit_list::plan_insert_newline_smart_list_aware(buffer)
}

pub(crate) fn leading_whitespace_for(rope: &Rope, line: usize) -> String {
    leading_whitespace(rope, line)
}

// Bullet toggling (`Ctrl+R` and `Ctrl+Shift+R`) is a self-contained
// sub-feature; it lives in `edit_lines/toggle_bullet.rs` so this file stays
// under the 600-line cap. Re-exported so `selection_edit.rs` keeps its
// import paths.
mod toggle_bullet;
pub(crate) use toggle_bullet::{
    plan_toggle_bullet_at_line_start, plan_toggle_bullet_with_continuation_indent,
};

pub(crate) fn plan_delete_to_line_start(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let head = selection.head;
        let line = head.line as usize;
        let line_start = rope.line_to_byte(line);
        let head_byte = head.to_byte_offset(rope)?;
        if head_byte > line_start {
            specs.push(EditSpec::delete(rope, line_start, head_byte)?);
            selections_after.push(Selection::caret_at(Position::new(line as u32, 0)));
        } else {
            selections_after.push(*selection);
        }
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_delete_to_line_end(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        let head = selection.head;
        let line = head.line as usize;
        let end = line_content_end(rope, line);
        let head_byte = head.to_byte_offset(rope)?;
        if end > head_byte {
            specs.push(EditSpec::delete(rope, head_byte, end)?);
        }
        selections_after.push(*selection);
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_duplicate_line(buffer: &Buffer) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_covered(buffer);
    if lines.is_empty() {
        return Ok(None);
    }
    let mut specs = Vec::new();
    for &line in &lines {
        let start = rope.line_to_byte(line);
        let end = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };
        let mut text = rope.byte_slice(start..end).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        specs.push(EditSpec::insert(rope, end, text)?);
    }
    let selections_after = selections_before.clone();
    Ok(finalize_specs(specs, selections_before, selections_after))
}

pub(crate) fn plan_duplicate_selection(
    buffer: &Buffer,
) -> Result<Option<SelectionEditPlan>, Error> {
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let mut specs = Vec::new();
    let mut selections_after = Vec::new();
    for selection in &selections_before {
        if selection.is_collapsed() {
            selections_after.push(*selection);
            continue;
        }
        let range = selection.ordered_range();
        let start = range.start.to_byte_offset(rope)?;
        let end = range.end.to_byte_offset(rope)?;
        let text = rope.byte_slice(start..end).to_string();
        specs.push(EditSpec::insert(rope, end, text)?);
        selections_after.push(*selection);
    }
    Ok(finalize_specs(specs, selections_before, selections_after))
}

// Line-block movement + join extracted to `edit_lines_movement.rs`; re-export so
// existing callers (selection_edit.rs) keep their import paths.
pub(crate) use crate::edit_lines_movement::{
    plan_join_lines, plan_join_selected_lines, plan_move_line_down, plan_move_line_up,
};

/// Rewrite the contiguous block of lines covered by selections by mapping
/// the original line list through `f`. Returns `None` when the rewrite is
/// a no-op or no lines are covered. Used by line-text ops in
/// `edit_line_text.rs`.
pub(crate) fn rewrite_covered_lines<F>(
    buffer: &Buffer,
    f: F,
) -> Result<Option<SelectionEditPlan>, Error>
where
    F: FnOnce(Vec<String>) -> Vec<String>,
{
    let rope = buffer.rope();
    let selections_before = buffer.selections().to_vec();
    let lines = lines_covered(buffer);
    if lines.is_empty() {
        return Ok(None);
    }
    let first = *lines.first().expect("non-empty");
    let last = *lines.last().expect("non-empty");
    let block_start = rope.line_to_byte(first);
    let block_end = if last + 1 < rope.len_lines() {
        rope.line_to_byte(last + 1)
    } else {
        rope.len_bytes()
    };
    let block_text = rope.byte_slice(block_start..block_end).to_string();
    let trailing_newline = block_text.ends_with('\n');
    let body = if trailing_newline {
        &block_text[..block_text.len() - 1]
    } else {
        &block_text[..]
    };
    let original: Vec<String> = body.split('\n').map(str::to_string).collect();
    let rewritten = f(original.clone());
    if rewritten == original {
        return Ok(None);
    }
    let mut combined = rewritten.join("\n");
    if trailing_newline {
        combined.push('\n');
    }
    let specs = vec![EditSpec::replace(rope, block_start, block_end, combined)?];
    Ok(finalize_specs(
        specs,
        selections_before.clone(),
        selections_before,
    ))
}

/// Sorted, deduplicated set of line numbers covered by the buffer's
/// selections. Empty when the buffer has no selections (which the buffer
/// itself prevents).
pub(crate) fn lines_covered(buffer: &Buffer) -> Vec<usize> {
    let mut lines: Vec<usize> = Vec::new();
    let len_lines = buffer.rope().len_lines();
    for selection in buffer.selections() {
        let range = selection.ordered_range();
        let start = range.start.line as usize;
        let end = range.end.line as usize;
        for line in start..=end {
            if line < len_lines && !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    lines.sort_unstable();
    lines
}

fn leading_whitespace(rope: &Rope, line: usize) -> String {
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let text = rope.byte_slice(start..end).to_string();
    let count: usize = text
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(char::len_utf8)
        .sum();
    text[..count].to_string()
}

#[cfg(test)]
mod tests;
