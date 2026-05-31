//! Atomic edit operations.
//!
//! Every mutation, every undo entry, and every persisted edit row reduces to
//! one of these three variants. Higher-level commands (move-line-up,
//! duplicate-selection, etc.) decompose into a sequence of `EditOp`s grouped
//! by an `UndoGroupId`.

use crate::{Position, Range};

/// A single, atomic edit on a buffer.
///
/// All positions and ranges are raw source-rope UTF-8 byte coordinates.
/// Display-map adjusted coordinates must be converted before constructing
/// an edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOp {
    /// Insert `text` at position `at`.
    Insert {
        /// Where to insert.
        at: Position,
        /// What to insert.
        text: String,
    },
    /// Delete the bytes covered by `range`.
    Delete {
        /// The range to delete.
        range: Range,
    },
    /// Replace the bytes covered by `range` with `text`.
    Replace {
        /// The range to replace.
        range: Range,
        /// The replacement text.
        text: String,
    },
}

impl EditOp {
    /// Convenience constructor for [`EditOp::Insert`].
    pub fn insert(at: Position, text: impl Into<String>) -> Self {
        Self::Insert {
            at,
            text: text.into(),
        }
    }

    /// Convenience constructor for [`EditOp::Delete`].
    #[must_use]
    pub fn delete(range: Range) -> Self {
        Self::Delete { range }
    }

    /// Convenience constructor for [`EditOp::Replace`].
    pub fn replace(range: Range, text: impl Into<String>) -> Self {
        Self::Replace {
            range,
            text: text.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_constructor() {
        let e = EditOp::insert(Position::ZERO, "hi");
        assert!(matches!(e, EditOp::Insert { ref text, .. } if text == "hi"));
    }

    #[test]
    fn delete_constructor() {
        let e = EditOp::delete(Range::empty(Position::ZERO));
        assert!(matches!(e, EditOp::Delete { .. }));
    }

    #[test]
    fn replace_constructor() {
        let e = EditOp::replace(Range::empty(Position::ZERO), "x");
        assert!(matches!(e, EditOp::Replace { ref text, .. } if text == "x"));
    }
}
