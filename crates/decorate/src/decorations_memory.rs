//! Memory estimates for cached [`crate::Decorations`] snapshots.
//!
//! Thread ownership follows the owner of the `Decorations`: worker
//! thread while building, UI thread after insertion into
//! [`crate::DecorationCache`].

use std::mem::size_of;

use crate::inline::InlineKind;
use crate::table_eval::{EvaluatedTable, TableCellOverride};
use crate::{Decorations, InlineColorSpan, InlineSpan};

const ESTIMATED_TREE_BYTES_PER_BLOCK: usize = 96;

impl Decorations {
    /// Estimated heap bytes retained by this decoration snapshot.
    ///
    /// `Decorations` does not retain the tree-sitter `Tree`; worker
    /// caches own those. The estimate still reserves a block-count
    /// proxy for the parse-tree footprint so memory traces can move
    /// with markdown structure.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        self.blocks.capacity() * size_of::<crate::BlockSpan>()
            + self.inlines.capacity() * size_of::<InlineSpan>()
            + self.inline_color_spans.capacity() * size_of::<InlineColorSpan>()
            + self.evaluated_tables.capacity() * size_of::<EvaluatedTable>()
            + self
                .inlines
                .iter()
                .map(|span| inline_kind_heap_bytes(&span.kind))
                .sum::<usize>()
            + self
                .evaluated_tables
                .iter()
                .map(evaluated_table_heap_bytes)
                .sum::<usize>()
            + self.blocks.len() * ESTIMATED_TREE_BYTES_PER_BLOCK
    }
}

fn inline_kind_heap_bytes(kind: &InlineKind) -> usize {
    match kind {
        InlineKind::FootnoteReference { label } => label.capacity(),
        InlineKind::FootnoteDefinition { label, .. } => label.capacity(),
        _ => 0,
    }
}

fn evaluated_table_heap_bytes(table: &EvaluatedTable) -> usize {
    table.overrides.capacity() * size_of::<TableCellOverride>()
        + table
            .overrides
            .iter()
            .map(|override_cell| override_cell.display.capacity())
            .sum::<usize>()
}
