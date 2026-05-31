//! Shared planning primitives used by every per-family planner module.
//!
//! `EditSpec` is the byte-level intermediate; `finalize_specs` sorts and
//! emits the descending op list with the post-edit selection set.

use continuity_text::{EditOp, Position, Range, Selection, SelectionKind};
use ropey::Rope;

use crate::selection_edit::SelectionEditPlan;
use crate::Error;

/// A pending byte-level edit. Sibling planner modules build vectors of
/// these and hand them to [`finalize_specs`] for ordering and op emission.
#[derive(Clone)]
pub struct EditSpec {
    pub start: usize,
    pub end: usize,
    pub start_position: Position,
    pub end_position_in_rope: Option<Position>,
    pub inserted: String,
}

impl EditSpec {
    /// Build a delete spec from a byte range plus the rope.
    pub(crate) fn delete(rope: &Rope, start: usize, end: usize) -> Result<Self, Error> {
        Ok(Self {
            start,
            end,
            start_position: Position::from_byte_offset(rope, start)?,
            end_position_in_rope: Some(Position::from_byte_offset(rope, end)?),
            inserted: String::new(),
        })
    }

    /// Build an insert spec at `at` of `text`.
    pub fn insert(rope: &Rope, at: usize, text: String) -> Result<Self, Error> {
        let position = Position::from_byte_offset(rope, at)?;
        Ok(Self {
            start: at,
            end: at,
            start_position: position,
            end_position_in_rope: Some(position),
            inserted: text,
        })
    }

    /// Build a replace spec covering `start..end` with `text`.
    pub(crate) fn replace(
        rope: &Rope,
        start: usize,
        end: usize,
        text: String,
    ) -> Result<Self, Error> {
        Ok(Self {
            start,
            end,
            start_position: Position::from_byte_offset(rope, start)?,
            end_position_in_rope: Some(Position::from_byte_offset(rope, end)?),
            inserted: text,
        })
    }

    pub(crate) fn into_op(self) -> EditOp {
        let range = Range::new(self.start_position, self.end_position());
        if self.start == self.end {
            EditOp::insert(self.start_position, self.inserted)
        } else if self.inserted.is_empty() {
            EditOp::delete(range)
        } else {
            EditOp::replace(range, self.inserted)
        }
    }

    fn end_position(&self) -> Position {
        self.end_position_in_rope.unwrap_or(self.start_position)
    }
}

/// Sort, merge overlaps, attach final selections, and emit the descending
/// op list. `selections_before` is recorded verbatim from the caller.
pub(crate) fn finalize_specs(
    mut specs: Vec<EditSpec>,
    selections_before: Vec<Selection>,
    selections_after: Vec<Selection>,
) -> Option<SelectionEditPlan> {
    if specs.is_empty() {
        return None;
    }
    specs.sort_by_key(|spec| spec.start);
    let merged = merge_specs(specs);
    let ops = merged.into_iter().rev().map(EditSpec::into_op).collect();
    Some(SelectionEditPlan {
        ops,
        selections_before,
        selections_after,
    })
}

/// Visible byte ranges for a selection, including per-line ranges for
/// block-wise selections.
pub(crate) fn ranges_for_selection(rope: &Rope, selection: Selection) -> Result<Vec<Range>, Error> {
    if selection.kind != SelectionKind::BlockWise || selection.is_collapsed() {
        return Ok(vec![selection.ordered_range()]);
    }
    let start_line = selection.anchor.line.min(selection.head.line);
    let end_line = selection.anchor.line.max(selection.head.line);
    let start_col = selection
        .anchor
        .byte_in_line
        .min(selection.head.byte_in_line);
    let end_col = selection
        .anchor
        .byte_in_line
        .max(selection.head.byte_in_line);
    let mut ranges = Vec::new();
    for line in start_line..=end_line {
        let start = line_column(rope, line, start_col)?;
        let end = line_column(rope, line, end_col)?;
        ranges.push(Range::new(start, end));
    }
    Ok(ranges)
}

fn line_column(rope: &Rope, line: u32, column: u32) -> Result<Position, Error> {
    let line = (line as usize).min(rope.len_lines().saturating_sub(1));
    let start = rope.line_to_byte(line);
    let end = line_content_end(rope, line);
    let byte = (start + column as usize).min(end);
    Ok(Position::from_byte_offset(rope, byte)?)
}

/// End-of-content byte for `line`, excluding any `\n` / `\r\n`.
pub(crate) fn line_content_end(rope: &Rope, line: usize) -> usize {
    let start = rope.line_to_byte(line);
    let next = if line + 1 < rope.len_lines() {
        rope.line_to_byte(line + 1)
    } else {
        rope.len_bytes()
    };
    let mut end = next;
    let slice = rope.byte_slice(start..next).to_string();
    if slice.ends_with('\n') {
        end = end.saturating_sub(1);
        if slice.ends_with("\r\n") {
            end = end.saturating_sub(1);
        }
    }
    end
}

pub(crate) fn caret_delete_range(
    rope: &Rope,
    byte: usize,
    backward: bool,
) -> Option<(usize, usize)> {
    if backward {
        if byte == 0 {
            return None;
        }
        let char_idx = rope.byte_to_char(byte);
        Some((rope.char_to_byte(char_idx.saturating_sub(1)), byte))
    } else {
        if byte >= rope.len_bytes() {
            return None;
        }
        let char_idx = rope.byte_to_char(byte);
        let next = (char_idx + 1).min(rope.len_chars());
        Some((byte, rope.char_to_byte(next)))
    }
}

pub(crate) fn merge_specs(specs: Vec<EditSpec>) -> Vec<EditSpec> {
    let mut merged: Vec<EditSpec> = Vec::new();
    for spec in specs {
        if let Some(last) = merged.last_mut() {
            if spec.start <= last.end && last.inserted.is_empty() && spec.inserted.is_empty() {
                last.end = last.end.max(spec.end);
                last.end_position_in_rope = spec.end_position_in_rope;
                continue;
            }
        }
        merged.push(spec);
    }
    merged
}

/// Advance `position` byte-wise by the bytes of `text`, treating `\n` as a
/// line break.
pub(crate) fn advance_position(mut position: Position, text: &str) -> Position {
    for ch in text.chars() {
        if ch == '\n' {
            position.line = position.line.saturating_add(1);
            position.byte_in_line = 0;
        } else {
            position.byte_in_line = position.byte_in_line.saturating_add(ch.len_utf8() as u32);
        }
    }
    position
}
