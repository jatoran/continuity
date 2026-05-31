//! Shift cached decoration byte ranges forward through edits applied
//! since the snapshot was computed.
//!
//! The decoration worker re-parses a markdown buffer asynchronously;
//! on a multi-thousand-line document the round trip is many tens of
//! milliseconds, so the UI's cached [`crate::Decorations`] is often a
//! handful of revisions behind the rope the renderer is about to
//! draw. Painting with the stale ranges as-is misaligns marker hides,
//! heading bounds, table layout, etc. for everything below the most
//! recent edit; when the worker catches up the rendering snaps back
//! into place. Users see "the whole document re-renders every
//! keystroke".
//!
//! This module's [`Decorations::transformed_through`] threads each
//! span through a slice of [`continuity_text::RopeEditDelta`]s
//! (chronological order from the snapshot's revision forward) and
//! returns a new [`Decorations`] whose byte ranges are aligned to
//! the post-edit rope. Spans entirely before the first edit are kept
//! unchanged; spans entirely after are shifted by the accumulated
//! byte delta; spans that intersect any edit are dropped — the
//! worker will produce correct ranges for the edited region soon
//! enough.

use std::ops::Range;

use continuity_text::{
    transform_container_range_through_chain, transform_range_through_chain, RopeEditDelta,
};

use crate::inline::{ByteRange, InlineSpan};
use crate::inline_color::InlineColorSpan;
use crate::spans::BlockSpan;
use crate::table_eval::{EvaluatedTable, TableCellOverride};
use crate::Decorations;

impl Decorations {
    /// Return a clone of `self` with every byte range remapped through
    /// the deltas in `deltas` (chronological order). The returned
    /// `Decorations` carries `new_revision` so a downstream
    /// equality-on-revision check (`cached.revision ==
    /// rope.revision`) sees a match.
    ///
    /// Spans that intersect any delta are dropped — the decoration
    /// worker will replace them with byte-correct ones when it
    /// catches up. Spans entirely before or after every delta survive
    /// (shifted as appropriate).
    ///
    /// Calling with an empty `deltas` slice is equivalent to
    /// `clone()` with the revision rewritten.
    #[must_use]
    pub fn transformed_through(&self, deltas: &[RopeEditDelta], new_revision: u64) -> Self {
        if deltas.is_empty() {
            let mut out = self.clone();
            out.revision = new_revision;
            return out;
        }
        let blocks = self
            .blocks
            .iter()
            .filter_map(|b| {
                let (start, end) = transform_range_through_chain(b.start_byte, b.end_byte, deltas)?;
                Some(BlockSpan {
                    kind: b.kind,
                    start_byte: start,
                    end_byte: end,
                })
            })
            .collect();
        let inlines = self
            .inlines
            .iter()
            .filter_map(|s| {
                let (start, end) =
                    transform_range_through_chain(s.range.start, s.range.end, deltas)?;
                Some(InlineSpan {
                    kind: s.kind.clone(),
                    range: ByteRange { start, end },
                })
            })
            .collect();
        let inline_color_spans = self
            .inline_color_spans
            .iter()
            .filter_map(|s| {
                let (outer_s, outer_e) =
                    transform_range_through_chain(s.outer.start, s.outer.end, deltas)?;
                let (inner_s, inner_e) =
                    transform_range_through_chain(s.inner.start, s.inner.end, deltas)?;
                Some(InlineColorSpan {
                    outer: Range {
                        start: outer_s,
                        end: outer_e,
                    },
                    inner: Range {
                        start: inner_s,
                        end: inner_e,
                    },
                    kind: s.kind,
                })
            })
            .collect();
        let evaluated_tables = self
            .evaluated_tables
            .iter()
            .filter_map(|t| {
                // Container semantics: a cell being typed into is an
                // edit *interior* to the table's block_range. The
                // plain range transform drops such a span, which makes
                // the table flicker to raw markdown on every keystroke
                // while the decoration worker lags a revision behind
                // (both the display-map hide pass and the chrome
                // painter key off `evaluated_tables`). Extending the
                // block_range's end through the interior edit keeps the
                // table alive for the lag frame; only a structural edit
                // straddling the block boundary drops it.
                let (br_s, br_e) = transform_container_range_through_chain(
                    t.block_range.start,
                    t.block_range.end,
                    deltas,
                )?;
                let overrides = t
                    .overrides
                    .iter()
                    .filter_map(|ov| {
                        let (s, e) = transform_range_through_chain(
                            ov.cell_range.start,
                            ov.cell_range.end,
                            deltas,
                        )?;
                        Some(TableCellOverride {
                            cell: ov.cell,
                            cell_range: Range { start: s, end: e },
                            display: ov.display.clone(),
                        })
                    })
                    .collect();
                Some(EvaluatedTable {
                    block_range: Range {
                        start: br_s,
                        end: br_e,
                    },
                    overrides,
                })
            })
            .collect();
        Self {
            revision: new_revision,
            blocks,
            inlines,
            inline_color_spans,
            evaluated_tables,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline::InlineKind;
    use crate::inline_color::InlineColorKind;
    use crate::spans::BlockKind;

    fn block(start: usize, end: usize) -> BlockSpan {
        BlockSpan {
            kind: BlockKind::Paragraph,
            start_byte: start,
            end_byte: end,
        }
    }

    fn inline(start: usize, end: usize) -> InlineSpan {
        InlineSpan {
            kind: InlineKind::Emphasis,
            range: ByteRange { start, end },
        }
    }

    fn decorations_with(
        revision: u64,
        blocks: Vec<BlockSpan>,
        inlines: Vec<InlineSpan>,
    ) -> Decorations {
        Decorations {
            revision,
            blocks,
            inlines,
            inline_color_spans: Vec::new(),
            evaluated_tables: Vec::new(),
        }
    }

    #[test]
    fn empty_deltas_only_rewrite_revision() {
        let d = decorations_with(7, vec![block(0, 10)], vec![inline(2, 5)]);
        let out = d.transformed_through(&[], 8);
        assert_eq!(out.revision, 8);
        assert_eq!(out.blocks, d.blocks);
        assert_eq!(out.inlines, d.inlines);
    }

    #[test]
    fn span_after_insertion_shifts() {
        let d = decorations_with(1, vec![block(100, 200)], vec![inline(150, 160)]);
        let out = d.transformed_through(&[RopeEditDelta::insert(50, 3)], 2);
        assert_eq!(out.blocks, vec![block(103, 203)]);
        assert_eq!(out.inlines, vec![inline(153, 163)]);
    }

    #[test]
    fn span_before_insertion_unchanged() {
        let d = decorations_with(1, vec![block(10, 20)], vec![inline(12, 18)]);
        let out = d.transformed_through(&[RopeEditDelta::insert(50, 3)], 2);
        assert_eq!(out.blocks, vec![block(10, 20)]);
        assert_eq!(out.inlines, vec![inline(12, 18)]);
    }

    #[test]
    fn span_overlapping_insertion_drops() {
        let d = decorations_with(1, vec![block(40, 60), block(100, 200)], vec![]);
        let out = d.transformed_through(&[RopeEditDelta::insert(50, 3)], 2);
        // First block straddled the insertion → dropped. Second
        // sits entirely after → kept + shifted.
        assert_eq!(out.blocks, vec![block(103, 203)]);
    }

    #[test]
    fn multi_delta_chain_accumulates() {
        let d = decorations_with(1, vec![block(100, 200)], vec![]);
        let chain = [
            RopeEditDelta::insert(10, 5), // +5 before the span
            RopeEditDelta::delete(20, 2), // post-shift delete still before the span
        ];
        let out = d.transformed_through(&chain, 3);
        assert_eq!(out.blocks, vec![block(103, 203)]);
    }

    fn decorations_with_table(revision: u64, table: EvaluatedTable) -> Decorations {
        Decorations {
            revision,
            blocks: Vec::new(),
            inlines: Vec::new(),
            inline_color_spans: Vec::new(),
            evaluated_tables: vec![table],
        }
    }

    #[test]
    fn table_survives_interior_edit_with_extended_block_range() {
        // Regression: typing inside a table cell is an interior edit.
        // The table must NOT be dropped (which flickered it to raw
        // markdown every keystroke while the worker lagged); its
        // block_range end extends through the insertion instead.
        let d = decorations_with_table(
            1,
            EvaluatedTable {
                block_range: 100..200,
                overrides: Vec::new(),
            },
        );
        let out = d.transformed_through(&[RopeEditDelta::insert(140, 1)], 2);
        assert_eq!(out.evaluated_tables.len(), 1, "table must survive");
        assert_eq!(out.evaluated_tables[0].block_range, 100..201);
    }

    #[test]
    fn table_dropped_when_fully_deleted() {
        // Selecting a whole table and deleting it collapses the range;
        // the table must drop so the chrome painter's delete-lag path
        // fires (no ghost chrome over blank source).
        let d = decorations_with_table(
            1,
            EvaluatedTable {
                block_range: 100..200,
                overrides: Vec::new(),
            },
        );
        let out = d.transformed_through(&[RopeEditDelta::delete(100, 100)], 2);
        assert!(
            out.evaluated_tables.is_empty(),
            "fully-deleted table must drop, got {:?}",
            out.evaluated_tables
        );
    }

    #[test]
    fn table_override_intersecting_edit_drops_but_table_kept() {
        // A formula override whose cell is being edited goes stale and
        // must drop (raw formula source re-shows until reparse), but
        // the surrounding table stays alive.
        let d = decorations_with_table(
            1,
            EvaluatedTable {
                block_range: 100..200,
                overrides: vec![TableCellOverride {
                    cell: crate::CellRef { col: 0, row: 0 },
                    cell_range: 130..140,
                    display: "9".into(),
                }],
            },
        );
        // Edit lands inside the override's cell_range → override drops.
        let out = d.transformed_through(&[RopeEditDelta::replace(132, 3, 1)], 2);
        assert_eq!(out.evaluated_tables.len(), 1, "table must survive");
        assert!(
            out.evaluated_tables[0].overrides.is_empty(),
            "stale override must drop, got {:?}",
            out.evaluated_tables[0].overrides
        );
    }

    #[test]
    fn inline_color_outer_and_inner_both_transform() {
        let span = InlineColorSpan {
            outer: 100..120,
            inner: 102..118,
            kind: InlineColorKind::Highlight,
        };
        let d = Decorations {
            revision: 1,
            blocks: Vec::new(),
            inlines: Vec::new(),
            inline_color_spans: vec![span],
            evaluated_tables: Vec::new(),
        };
        let out = d.transformed_through(&[RopeEditDelta::insert(50, 4)], 2);
        assert_eq!(out.inline_color_spans.len(), 1);
        let span = &out.inline_color_spans[0];
        assert_eq!(span.outer, 104..124);
        assert_eq!(span.inner, 106..122);
    }
}
