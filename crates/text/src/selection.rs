//! Selections: `(anchor, head, kind)`.
//!
//! Multi-cursor is the general case; a single cursor is a `Vec<Selection>`
//! of length 1. Block (column) selection is a real selection kind because
//! it preserves through edits in a way multi-cursors cannot.

use serde::{Deserialize, Serialize};

use crate::{Position, Range};

/// The flavor of a selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelectionKind {
    /// A caret with no extent (`anchor == head`).
    Caret,
    /// A line-wise selection (extends to whole lines).
    LineWise,
    /// A block / column selection.
    BlockWise,
}

/// A single cursor or selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Selection {
    /// The anchor (the end that does not move when extending).
    pub anchor: Position,
    /// The head (the moving end / where the caret is rendered).
    pub head: Position,
    /// The selection kind.
    pub kind: SelectionKind,
}

impl Selection {
    /// Construct a selection from its anchor, head, and kind.
    #[must_use]
    pub const fn new(anchor: Position, head: Position, kind: SelectionKind) -> Self {
        Self { anchor, head, kind }
    }

    /// A caret at `p` (no extent).
    #[must_use]
    pub const fn caret_at(p: Position) -> Self {
        Self {
            anchor: p,
            head: p,
            kind: SelectionKind::Caret,
        }
    }

    /// `true` when the selection has zero extent.
    #[must_use]
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.head
    }

    /// `true` when the selection is a caret (kind = Caret OR collapsed).
    #[must_use]
    pub fn is_caret(&self) -> bool {
        matches!(self.kind, SelectionKind::Caret) && self.is_collapsed()
    }

    /// Return the character-wise range covered by this selection.
    #[must_use]
    pub fn range(&self) -> Range {
        Range::new(self.anchor, self.head)
    }

    /// Return the covered range with `start <= end`.
    #[must_use]
    pub fn ordered_range(&self) -> Range {
        self.range().ordered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_at_origin_is_collapsed_and_caret() {
        let s = Selection::caret_at(Position::ZERO);
        assert!(s.is_collapsed());
        assert!(s.is_caret());
    }

    #[test]
    fn linewise_with_extent_is_not_caret() {
        let s = Selection {
            anchor: Position::ZERO,
            head: Position::new(2, 0),
            kind: SelectionKind::LineWise,
        };
        assert!(!s.is_collapsed());
        assert!(!s.is_caret());
    }

    #[test]
    fn caret_kind_with_extent_is_not_caret() {
        let s = Selection {
            anchor: Position::ZERO,
            head: Position::new(0, 3),
            kind: SelectionKind::Caret,
        };
        assert!(!s.is_collapsed());
        assert!(!s.is_caret());
        assert_eq!(s.ordered_range().start, Position::ZERO);
        assert_eq!(s.ordered_range().end, Position::new(0, 3));
    }
}
