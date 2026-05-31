//! Per-edit selection bookkeeping used by [`crate::Buffer::apply`].
//!
//! When the rope mutates, selection endpoints that lay across the
//! edited range must shift in lockstep so the cursor stays where the
//! writer expects it. The math is the same for caret, line-wise, and
//! block-wise selections: any byte offset before the edit point is
//! unchanged, any byte offset inside the removed range collapses to
//! the edit's new end, and any byte offset after the edit point shifts
//! by the size delta.

use continuity_text::{EditOp, Position, Selection};
use ropey::Rope;

use crate::Error;

pub(super) struct SelectionTransform {
    start: usize,
    old_end: usize,
    new_end: usize,
}

impl SelectionTransform {
    pub(super) fn from_op(rope: &Rope, op: &EditOp) -> Result<Self, Error> {
        match op {
            EditOp::Insert { at, text } => {
                let start = at.to_byte_offset(rope)?;
                Ok(Self {
                    start,
                    old_end: start,
                    new_end: start.saturating_add(text.len()),
                })
            }
            EditOp::Delete { range } => {
                let start = range.start.to_byte_offset(rope)?;
                let old_end = range.end.to_byte_offset(rope)?;
                Ok(Self {
                    start,
                    old_end,
                    new_end: start,
                })
            }
            EditOp::Replace { range, text } => {
                let start = range.start.to_byte_offset(rope)?;
                let old_end = range.end.to_byte_offset(rope)?;
                Ok(Self {
                    start,
                    old_end,
                    new_end: start.saturating_add(text.len()),
                })
            }
        }
    }

    pub(super) fn apply_all(
        &self,
        old_rope: &Rope,
        new_rope: &Rope,
        selections: &[Selection],
    ) -> Vec<Selection> {
        selections
            .iter()
            .map(|selection| Selection {
                anchor: self.apply_position(old_rope, new_rope, selection.anchor),
                head: self.apply_position(old_rope, new_rope, selection.head),
                kind: selection.kind,
            })
            .collect()
    }

    fn apply_position(&self, old_rope: &Rope, new_rope: &Rope, position: Position) -> Position {
        let old_byte = position.to_byte_offset(old_rope).unwrap_or(0);
        let new_byte = if old_byte < self.start {
            old_byte
        } else if old_byte <= self.old_end {
            self.new_end
        } else {
            old_byte
                .saturating_sub(self.old_end.saturating_sub(self.start))
                .saturating_add(self.new_end.saturating_sub(self.start))
        };
        Position::from_byte_offset(new_rope, new_byte.min(new_rope.len_bytes()))
            .unwrap_or(Position::ZERO)
    }
}
