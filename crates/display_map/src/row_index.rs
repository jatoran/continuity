//! Display-row index — whole-document row-count table that backs the
//! `DisplayMap`'s offscreen queries (scrollbar height, EOF visibility,
//! caret anchoring, hit-test, source↔display mapping) without storing
//! a [`crate::DisplayLineSpec`] per row.
//!
//! ## Why it exists
//!
//! Pre-ε.1, `DisplayMap` answered every offscreen query by indexing into
//! its `Vec<DisplayLineSpec>`. Realizing one spec costs O(line length)
//! for segments + soft-wrap measurement; the whole vector is O(source
//! lines × line cost). At ~6000 lines this dominates per-paint cost.
//! Every offscreen consumer (scrollbar, EOF, caret anchor, hit-test,
//! fold/wrap/image-reservation math) reads from the same vector, so the
//! viewport-only realization arriving in ε.2 cannot simply skip
//! offscreen specs — those consumers still need answers.
//!
//! `DisplayRowIndex` is the answer-bearer. It covers the entire document
//! at constant memory per source line (`u16` count + Fenwick partial sum)
//! and answers the offscreen queries in O(log n) without owning any
//! `DisplayLineSpec`s. ε.1 wires the index in as the source of truth for
//! these queries; ε.2 reduces the realized-spec vector to viewport-only.
//!
//! ## Stamping
//!
//! An index is keyed against the inputs it was built from
//! ([`IndexStamps`]): rope revision, decoration revision, soft-wrap
//! width, opaque font-state hash, and an opaque fold-signature hash.
//! ε.3's per-line dirty invalidation compares stamps to decide which
//! lines must be rebuilt; an arbitrary stamp drift forces a full
//! rebuild.
//!
//! ## Thread ownership
//!
//! Constructed on the same worker that builds the parent `DisplayMap`,
//! then handed to the UI thread inside `Arc<DisplayMap>`. The index is
//! immutable post-construction (ε.3 will add an in-place `edit` path
//! mutating it on the projection worker).

use std::fmt;
use std::ops::Range;

use crate::row_index_fenwick::Fenwick;

mod bracket;
pub mod dirty;
mod lookup;
pub mod splice;

/// Inputs the index was built against. Stamps decay-validate the index
/// against the live editor state; ε.3 uses them to decide which source
/// lines need a row-count rebuild after a rope or decoration delta.
///
/// The font-state and fold-signature hashes are opaque to the index —
/// callers pick whatever fingerprint identifies "the inputs the row
/// counts depend on" in their domain.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Default)]
pub struct IndexStamps {
    /// Source rope revision at build time.
    pub rope_revision: u64,
    /// Decoration revision at build time.
    pub decoration_revision: u64,
    /// Soft-wrap width (DIP) at build time. `0` means wrap disabled.
    pub wrap_width_dip: u32,
    /// Opaque hash of the font-state inputs that affect row counts
    /// (font family/size, scale tier, line-height policy, ...). ε.3
    /// uses drift to trigger an index rebuild; ε.1 callers may leave
    /// this at `0`.
    pub font_state: u64,
    /// Opaque hash of the active fold set / image-reservation set.
    /// Same use as `font_state`.
    pub fold_signature: u64,
}

/// P18.5 — state carried by an index built from a viewport-priority
/// partial walk.
///
/// When present, the parent [`DisplayRowIndex`] holds *real* row counts
/// only for source lines inside `walked_source_range`; the rest are
/// placeholders (see
/// [`crate::builder::progressive_walker::UNWALKED_PLACEHOLDER_ROW_COUNT`]).
/// Scrollbar consumers read [`scrollbar_estimate`] via
/// [`DisplayRowIndex::estimated_total_rows`]; existing row-lookup paths
/// keep using the Fenwick prefix-sum tree unchanged.
///
/// `full_revision_target` carries the rope revision the background fill
/// should walk so the next paint epilogue can dedupe a completed full
/// index against the current paint's stamp.
///
/// [`scrollbar_estimate`]: PartialRowIndexState::scrollbar_estimate
#[derive(Clone, Debug, PartialEq)]
pub struct PartialRowIndexState {
    /// Source-line range walked at viewport-priority time. Inside this
    /// range, `row_counts` holds the real per-line counts the cold full
    /// walker would emit; outside, placeholder ones.
    pub walked_source_range: Range<u32>,
    /// Density-based estimate of the document's total display-row count.
    /// Returned by [`DisplayRowIndex::estimated_total_rows`] until the
    /// background fill completes and the next paint installs the full
    /// index.
    pub scrollbar_estimate: u32,
    /// Rope revision the background-fill request should walk. Equal to
    /// `stamps().rope_revision` at partial-walk time. Paint epilogue
    /// reads it to enqueue the background fill.
    pub full_revision_target: u64,
}

/// Display-row index over a source rope.
///
/// One slot per source line, holding the number of display rows that
/// source line projects to under the index's stamp tuple. Folded source
/// lines have a row count of `0`; the synthetic trailing empty line
/// produced by a final `\n` has a row count of `1` (matches the legacy
/// builder).
///
/// `partial_state` is `Some` when the index was constructed from a
/// viewport-priority walk (P18.5). Outside the walked range the
/// `row_counts` slot holds a placeholder rather than the real per-line
/// count; consumers that care about whole-document totals read
/// [`Self::estimated_total_rows`].
#[derive(Clone)]
pub struct DisplayRowIndex {
    /// Per-source-line row counts. `u16` caps at 65535 wrap continuation
    /// rows per source line — far above any realistic single-line
    /// soft-wrap fan-out.
    row_counts: Vec<u16>,
    /// Display-row prefix-sum tree over `row_counts`.
    prefix_sums: Fenwick,
    /// Stamps the index was built against.
    stamps: IndexStamps,
    /// Set when the index was constructed from a viewport-priority
    /// partial walk; `None` for cold full walks and splice / dirty
    /// rebuilds.
    partial_state: Option<PartialRowIndexState>,
}

impl DisplayRowIndex {
    /// Build the index from a slice of per-source-line row counts.
    ///
    /// `row_counts.len()` is the source line count of the underlying
    /// rope (including the synthetic trailing empty line emitted by a
    /// final `\n`).
    #[must_use]
    pub fn from_row_counts(row_counts: Vec<u16>, stamps: IndexStamps) -> Self {
        let prefix_input: Vec<u32> = row_counts.iter().copied().map(u32::from).collect();
        let prefix_sums = Fenwick::from_values(&prefix_input);
        Self {
            row_counts,
            prefix_sums,
            stamps,
            partial_state: None,
        }
    }

    /// P18.5 — build the index from a viewport-priority partial walk.
    ///
    /// `row_counts` holds real counts inside `partial.walked_source_range`
    /// and placeholders outside it; the supplied
    /// [`PartialRowIndexState`] is what
    /// [`Self::estimated_total_rows`] and [`Self::partial_state`] report.
    /// Consumers wishing to detect a partial index inspect
    /// [`Self::is_partial`].
    #[must_use]
    pub fn from_partial_row_counts(
        row_counts: Vec<u16>,
        stamps: IndexStamps,
        partial: PartialRowIndexState,
    ) -> Self {
        let prefix_input: Vec<u32> = row_counts.iter().copied().map(u32::from).collect();
        let prefix_sums = Fenwick::from_values(&prefix_input);
        Self {
            row_counts,
            prefix_sums,
            stamps,
            partial_state: Some(partial),
        }
    }

    /// Number of source lines in the index.
    #[must_use]
    pub fn source_line_count(&self) -> u32 {
        self.row_counts.len() as u32
    }

    /// Total number of display rows the index covers — the answer the
    /// scrollbar uses for content height.
    #[must_use]
    pub fn display_row_count(&self) -> u32 {
        self.prefix_sums.total() as u32
    }

    /// Borrow the stamps the index was built against.
    #[must_use]
    pub fn stamps(&self) -> &IndexStamps {
        &self.stamps
    }

    /// Replace the stamps the index reports. Used by ε.3's
    /// `rebuild_dirty` path to advance the rope/decoration revision
    /// after a successful in-place row-count update.
    pub fn set_stamps(&mut self, stamps: IndexStamps) {
        self.stamps = stamps;
    }

    /// Borrow the raw per-source-line counts (test instrumentation).
    #[must_use]
    pub fn row_counts(&self) -> &[u16] {
        &self.row_counts
    }

    /// `true` when the index was built from a viewport-priority partial
    /// walk (P18.5) and the background fill has not yet replaced it.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        self.partial_state.is_some()
    }

    /// Borrow the partial-walk state, if any.
    #[must_use]
    pub fn partial_state(&self) -> Option<&PartialRowIndexState> {
        self.partial_state.as_ref()
    }

    /// Estimated total display rows the document projects to.
    ///
    /// For a fully-walked index this equals
    /// [`Self::display_row_count`]. For a partial index it returns the
    /// density-based scrollbar estimate from the walked sample. Once
    /// the background fill installs the full index, the next paint
    /// reads the exact total.
    #[must_use]
    pub fn estimated_total_rows(&self) -> u32 {
        match &self.partial_state {
            Some(state) => state.scrollbar_estimate,
            None => self.display_row_count(),
        }
    }
}

impl fmt::Debug for DisplayRowIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DisplayRowIndex")
            .field("source_lines", &self.row_counts.len())
            .field("display_rows", &self.display_row_count())
            .field("stamps", &self.stamps)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamps() -> IndexStamps {
        IndexStamps::default()
    }

    #[test]
    fn empty_index_is_well_formed() {
        use crate::id::SourceLine;
        let index = DisplayRowIndex::from_row_counts(vec![], stamps());
        assert_eq!(index.source_line_count(), 0);
        assert_eq!(index.display_row_count(), 0);
        assert_eq!(index.source_line_for_display_row(0), None);
        // Querying any source line on an empty index points to row 0.
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(0)).raw(),
            0
        );
    }

    #[test]
    fn stamps_round_trip() {
        let s = IndexStamps {
            rope_revision: 42,
            decoration_revision: 7,
            wrap_width_dip: 480,
            font_state: 0xabcd,
            fold_signature: 0x1234,
        };
        let index = DisplayRowIndex::from_row_counts(vec![1, 2], s);
        assert_eq!(index.stamps(), &s);
    }
}
