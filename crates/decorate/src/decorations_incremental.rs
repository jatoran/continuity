//! Incremental tree-sitter parse support for [`crate::Decorations`].
//!
//! The decoration worker owns cached parse trees per buffer. This module
//! applies position-augmented rope deltas to a cached tree, parses once
//! against the edited tree, then extracts the same semantic decoration data
//! as the full parse path.

use continuity_text::RopeEditDelta;
use tree_sitter::{InputEdit, Point, Tree};

use crate::table_block_fixup::fill_empty_pipe_rows_for_parser;
use crate::{Decorations, MarkdownParser};

/// A `(row, column)` position for tree-sitter edits.
///
/// Both fields are 0-based. Columns are byte offsets into the line, matching
/// tree-sitter's `Point` semantics.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub struct EditPoint {
    /// 0-based line index.
    pub row: u32,
    /// 0-based byte offset into the line.
    pub column: u32,
}

impl EditPoint {
    /// Construct a point at `(row, column)`.
    #[must_use]
    pub const fn new(row: u32, column: u32) -> Self {
        Self { row, column }
    }
}

/// Byte-shift delta plus the tree-sitter points needed for `Tree::edit`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RopeEditDeltaWithPoints {
    /// Byte-level rope edit delta.
    pub delta: RopeEditDelta,
    /// Pre-edit position at `delta.at`.
    pub start_point: EditPoint,
    /// Pre-edit position at `delta.at + delta.removed_bytes`.
    pub old_end_point: EditPoint,
    /// Post-edit position at `delta.at + delta.inserted_bytes`.
    pub new_end_point: EditPoint,
}

impl Decorations {
    /// Compute decorations by applying `deltas_with_points` to `prev_tree`
    /// and parsing `source` once against the edited tree.
    ///
    /// Returns `None` if the parser cannot be constructed, the reparse fails,
    /// or the deltas' byte shifts do not reconcile `prev_source_len` with
    /// `source.len()`.
    #[must_use]
    pub fn compute_incremental(
        source: &str,
        revision: u64,
        prev_tree: &Tree,
        deltas_with_points: &[RopeEditDeltaWithPoints],
        prev_source_len: usize,
    ) -> Option<(Self, Tree)> {
        let expected_len = source_len_after_deltas(prev_source_len, deltas_with_points)?;
        if expected_len != source.len() {
            return None;
        }

        let mut edited_tree = prev_tree.clone();
        for delta in deltas_with_points {
            edited_tree.edit(&input_edit(delta));
        }

        let mut parser = MarkdownParser::new().ok()?;
        let parse_owned = fill_empty_pipe_rows_for_parser(source);
        let parse_str = parse_owned.as_deref().unwrap_or(source);
        let tree = parser.parse(parse_str, Some(&edited_tree))?;
        let decorations = Self::from_tree(source, revision, &tree);
        Some((decorations, tree))
    }
}

fn source_len_after_deltas(
    prev_source_len: usize,
    deltas: &[RopeEditDeltaWithPoints],
) -> Option<usize> {
    let mut len = isize::try_from(prev_source_len).ok()?;
    for delta in deltas {
        len = len.checked_add(delta.delta.shift())?;
        if len < 0 {
            return None;
        }
    }
    usize::try_from(len).ok()
}

fn input_edit(delta: &RopeEditDeltaWithPoints) -> InputEdit {
    InputEdit {
        start_byte: delta.delta.at,
        old_end_byte: delta.delta.at + delta.delta.removed_bytes,
        new_end_byte: delta.delta.at + delta.delta.inserted_bytes,
        start_position: point(delta.start_point),
        old_end_position: point(delta.old_end_point),
        new_end_position: point(delta.new_end_point),
    }
}

fn point(point: EditPoint) -> Point {
    Point::new(point.row as usize, point.column as usize)
}
