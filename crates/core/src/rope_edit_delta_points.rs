//! ε.4 — position-augmented rope edit deltas for the decoration
//! worker's incremental tree-sitter parse.
//!
//! `continuity_text::RopeEditDelta` is intentionally byte-only;
//! `decorations_transform.rs` and every other downstream consumer
//! that only needs to shift byte ranges through edits stays on that
//! minimal primitive. Tree-sitter's `Tree::edit` API, however,
//! requires `(row, column)` positions for the start, old end, and
//! new end of every edit — without them an incremental reparse
//! can't reuse subtrees correctly.
//!
//! Computing the three positions is cheap on the core thread: it
//! has the rope at edit time (just applied the op against it), so a
//! single `byte_to_line` + a subtraction yields each `EditPoint`.
//! Stashing them on `DeltaHistoryEntry` at capture time means the
//! decoration worker (running off-thread later) doesn't need to
//! re-derive positions from a stale or partial rope.
//!
//! ## Layer position
//!
//! Lives in `continuity_core` next to the rope edit it augments.
//! Consumers below `core` (e.g. `continuity_decorate`) receive a
//! representation that mirrors the same fields via the UI layer's
//! request producers — they do not depend on this crate.

use continuity_text::RopeEditDelta;
use ropey::Rope;

/// One `(row, column)` position into a rope. Both fields are 0-based
/// and counted in **bytes** to match tree-sitter's `Point` semantics
/// (tree-sitter columns are byte offsets, not character or UTF-16
/// units).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub struct EditPoint {
    /// 0-based line index.
    pub row: u32,
    /// 0-based byte offset into the line. **Bytes**, not characters
    /// or UTF-16 units (tree-sitter semantics).
    pub column: u32,
}

impl EditPoint {
    /// Construct a point at `(row, column)`.
    #[must_use]
    pub const fn new(row: u32, column: u32) -> Self {
        Self { row, column }
    }

    /// Compute the `EditPoint` of a byte position in `rope`. Clamps
    /// `byte` to the rope's length so a probe at exactly
    /// `rope.len_bytes()` lands on the row immediately past EOF
    /// (matching tree-sitter's expectation that `new_end_position`
    /// can sit at the very end of the document).
    #[must_use]
    pub fn from_rope_byte(rope: &Rope, byte: usize) -> Self {
        let len = rope.len_bytes();
        let clamped = byte.min(len);
        let row = rope.byte_to_line(clamped) as u32;
        let line_start = rope.line_to_byte(row as usize);
        let column = (clamped - line_start) as u32;
        Self { row, column }
    }
}

/// Byte-shift delta paired with the pre-edit and post-edit positions
/// tree-sitter needs to construct an `InputEdit`.
///
/// - `start_point` is `(row, column)` of `delta.at` in the **pre-edit**
///   rope.
/// - `old_end_point` is `(row, column)` of `delta.at + removed_bytes`
///   in the **pre-edit** rope.
/// - `new_end_point` is `(row, column)` of `delta.at + inserted_bytes`
///   in the **post-edit** rope.
///
/// Plan ops are captured in descending-byte order, so a chain walk
/// through them stays consistent with the rope's evolution — the
/// same invariant that `continuity_text::transform_range_through_chain`
/// relies on.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RopeEditDeltaWithPoints {
    /// Byte-shift component (identical to the
    /// [`continuity_text::RopeEditDelta`] the decorations-transform
    /// path uses).
    pub delta: RopeEditDelta,
    /// Pre-edit position at `delta.at`.
    pub start_point: EditPoint,
    /// Pre-edit position at `delta.at + delta.removed_bytes`.
    pub old_end_point: EditPoint,
    /// Post-edit position at `delta.at + delta.inserted_bytes`.
    pub new_end_point: EditPoint,
}

impl RopeEditDeltaWithPoints {
    /// Capture positions for `delta` against the pre-edit `rope` and
    /// the to-be-inserted `inserted_text`.
    ///
    /// The pre-edit rope supplies `start_point` and `old_end_point`
    /// directly. The post-edit `new_end_point` is computed by
    /// walking `inserted_text` for newlines: each `\n` (and `\r\n`)
    /// bumps the row; the column is the byte offset of the byte
    /// after the last newline in the inserted text, or
    /// `start_point.column + inserted_text.len()` when the insert
    /// contains no newlines.
    #[must_use]
    pub fn capture(delta: RopeEditDelta, rope: &Rope, inserted_text: &str) -> Self {
        let start_point = EditPoint::from_rope_byte(rope, delta.at);
        let old_end_point = EditPoint::from_rope_byte(rope, delta.at + delta.removed_bytes);
        let new_end_point = new_end_point_after_insert(start_point, inserted_text);
        Self {
            delta,
            start_point,
            old_end_point,
            new_end_point,
        }
    }
}

fn new_end_point_after_insert(start_point: EditPoint, inserted_text: &str) -> EditPoint {
    // Count newlines in the inserted text and find the byte offset
    // of the byte immediately after the LAST newline. Tree-sitter
    // wants the *post-edit* end position; for a pure single-line
    // insert that's `start_point.column + inserted_bytes`, but a
    // multi-line insert resets the column to "bytes since the last
    // \n inside the insert."
    let bytes = inserted_text.as_bytes();
    let mut row = start_point.row;
    let mut last_newline_end: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            row = row.saturating_add(1);
            last_newline_end = Some(i + 1);
        }
    }
    let column = match last_newline_end {
        Some(last_end) => (bytes.len() - last_end) as u32,
        None => start_point.column.saturating_add(bytes.len() as u32),
    };
    EditPoint { row, column }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    #[test]
    fn edit_point_for_start_of_first_line() {
        let r = rope("hello\nworld");
        let p = EditPoint::from_rope_byte(&r, 0);
        assert_eq!(p, EditPoint::new(0, 0));
    }

    #[test]
    fn edit_point_for_middle_of_first_line() {
        let r = rope("hello\nworld");
        let p = EditPoint::from_rope_byte(&r, 3);
        assert_eq!(p, EditPoint::new(0, 3));
    }

    #[test]
    fn edit_point_for_start_of_second_line() {
        // "hello\nworld" — newline at byte 5, line 1 starts at byte 6.
        let r = rope("hello\nworld");
        let p = EditPoint::from_rope_byte(&r, 6);
        assert_eq!(p, EditPoint::new(1, 0));
    }

    #[test]
    fn edit_point_clamped_at_eof() {
        let r = rope("ab\ncd");
        let p = EditPoint::from_rope_byte(&r, 999);
        // Clamped to rope.len_bytes() == 5; byte 5 is past EOF on
        // the synthetic trailing-empty line.
        assert_eq!(p.row, 1);
        assert!(p.column <= 2);
    }

    #[test]
    fn capture_single_line_insert() {
        // Pre: "abc\nxyz". Insert "Q" at byte 2 (between 'b' and 'c').
        let r = rope("abc\nxyz");
        let delta = RopeEditDelta::insert(2, 1);
        let captured = RopeEditDeltaWithPoints::capture(delta, &r, "Q");
        assert_eq!(captured.start_point, EditPoint::new(0, 2));
        assert_eq!(captured.old_end_point, EditPoint::new(0, 2));
        assert_eq!(captured.new_end_point, EditPoint::new(0, 3));
    }

    #[test]
    fn capture_multi_line_insert() {
        // Pre: "abc\nxyz". Insert "Q\nR" at byte 2.
        let r = rope("abc\nxyz");
        let delta = RopeEditDelta::insert(2, 3);
        let captured = RopeEditDeltaWithPoints::capture(delta, &r, "Q\nR");
        assert_eq!(captured.start_point, EditPoint::new(0, 2));
        assert_eq!(captured.old_end_point, EditPoint::new(0, 2));
        // Post-edit rope would be "abQ\nRc\nxyz". The new end is
        // immediately after "R" — row 1, column 1.
        assert_eq!(captured.new_end_point, EditPoint::new(1, 1));
    }

    #[test]
    fn capture_pure_delete_collapses_new_end_to_start() {
        // Pre: "abcdef". Delete bytes 2..4 → "abef".
        let r = rope("abcdef");
        let delta = RopeEditDelta::delete(2, 2);
        let captured = RopeEditDeltaWithPoints::capture(delta, &r, "");
        assert_eq!(captured.start_point, EditPoint::new(0, 2));
        assert_eq!(captured.old_end_point, EditPoint::new(0, 4));
        assert_eq!(captured.new_end_point, EditPoint::new(0, 2));
    }

    #[test]
    fn capture_replace_with_newline_in_inserted_text() {
        // Pre: "abc def". Replace bytes 3..4 (" ") with "\n" → "abc\ndef".
        let r = rope("abc def");
        let delta = RopeEditDelta::replace(3, 1, 1);
        let captured = RopeEditDeltaWithPoints::capture(delta, &r, "\n");
        assert_eq!(captured.start_point, EditPoint::new(0, 3));
        assert_eq!(captured.old_end_point, EditPoint::new(0, 4));
        // Inserted text "\n" — row bumps to 1, column resets to 0
        // (bytes after the trailing newline).
        assert_eq!(captured.new_end_point, EditPoint::new(1, 0));
    }
}
