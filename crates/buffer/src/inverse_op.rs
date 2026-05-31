//! Inverse-op derivation for undo recording.
//!
//! Given an `EditOp`, the substring it removed (empty for `Insert`),
//! and the post-edit rope, return the op that would undo it. Pure
//! logic; the recovery path uses it on replayed edits to reconstruct
//! undo records without having the pre-edit rope on hand.

use continuity_text::{EditOp, Position, Range};
use ropey::Rope;

use crate::Error;

/// Compute the inverse of `op` given the original removed text and the
/// post-edit rope. Pure logic; reusable by recovery (which has only
/// `(rope_after, op, removed_text)`) as well as by the core thread.
///
/// # Errors
///
/// Returns [`Error::Text`] when positions fall outside `rope_after`.
pub fn compute_inverse_op(
    op: &EditOp,
    removed_text: &str,
    rope_after: &Rope,
) -> Result<EditOp, Error> {
    match op {
        EditOp::Insert { at, text } => {
            let start_byte = at.to_byte_offset(rope_after)?;
            let end_byte = start_byte
                .saturating_add(text.len())
                .min(rope_after.len_bytes());
            let end = Position::from_byte_offset(rope_after, end_byte)?;
            Ok(EditOp::delete(Range::new(*at, end)))
        }
        EditOp::Delete { range } => Ok(EditOp::insert(range.start, removed_text.to_string())),
        EditOp::Replace { range, text } => {
            let start_byte = range.start.to_byte_offset(rope_after)?;
            let end_byte = start_byte
                .saturating_add(text.len())
                .min(rope_after.len_bytes());
            let end = Position::from_byte_offset(rope_after, end_byte)?;
            Ok(EditOp::replace(
                Range::new(range.start, end),
                removed_text.to_string(),
            ))
        }
    }
}
