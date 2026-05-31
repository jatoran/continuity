//! In-place mutators for [`DisplayRowIndex`] — single-slot updates
//! ([`DisplayRowIndex::set_row_count`]) and contiguous-slot splices
//! ([`DisplayRowIndex::splice_rows`]). The splice payload type
//! [`RowSplice`] is consumed by ε.3F's `rebuild_spliced` codepath.

use crate::id::SourceLine;
use crate::row_index_fenwick::Fenwick;

use super::DisplayRowIndex;

impl DisplayRowIndex {
    /// ε.3 — replace the row count for a single source line in place.
    /// Updates the Fenwick prefix tree so subsequent
    /// `prefix_sum` / `find_by_prefix` queries return the new totals
    /// in O(log n). Panics if `source_line` is out of range.
    pub fn set_row_count(&mut self, source_line: SourceLine, count: u16) {
        let i = source_line.as_usize();
        self.row_counts[i] = count;
        self.prefix_sums.set(i, u32::from(count));
    }

    /// ε.3F — splice row-count slots in place. Removes `splice.removed`
    /// slots starting at `splice.at` and inserts `splice.inserted`
    /// placeholder slots (each set to `default_count`) at the same
    /// position. Callers must follow up with `set_row_count` for every
    /// source-line index in `splice.dirty` so the placeholders are
    /// replaced by the real post-edit row counts.
    ///
    /// The Fenwick prefix tree is rebuilt from the resulting
    /// `row_counts` vector — O(n) in source-line count. That is the
    /// step-one trade-off documented in roadmap_v4 ε.3F: the splice
    /// avoids walking `row_count_for_source_line` over every source
    /// line (the dominant Enter-on-large-buffer cost), but pays an
    /// O(n) Fenwick rebuild. A future segment-tree replacement can
    /// bring this to O(log n).
    ///
    /// # Panics
    ///
    /// Panics in debug builds if the splice's `at + removed` overruns
    /// the current row count. Release builds clamp to the row-count
    /// length to keep behavior defined.
    pub fn splice_rows(&mut self, splice: &RowSplice, default_count: u16) {
        let at = (splice.at as usize).min(self.row_counts.len());
        let end = (at + splice.removed as usize).min(self.row_counts.len());
        debug_assert!(
            (splice.at as usize) + (splice.removed as usize) <= self.row_counts.len(),
            "splice_rows out of range: at={} removed={} len={}",
            splice.at,
            splice.removed,
            self.row_counts.len(),
        );
        let new_iter = std::iter::repeat_n(default_count, splice.inserted as usize);
        self.row_counts.splice(at..end, new_iter);
        let prefix_input: Vec<u32> = self.row_counts.iter().copied().map(u32::from).collect();
        self.prefix_sums = Fenwick::from_values(&prefix_input);
    }
}

/// ε.3F — payload for [`super::RowDirty::Splice`]. Describes one local
/// line-count edit at `at` (post-edit source-line index) that
/// removes `removed` slots and inserts `inserted` slots, then needs
/// row counts recomputed for every source line in `dirty`.
///
/// For a single-`\n` insert (Enter): `removed = 1`, `inserted = 2`
/// (one source line split into two). For a multi-line paste of `N`
/// newlines: `removed = 1`, `inserted = N + 1`. For a single-`\n`
/// delete: `removed = 2`, `inserted = 1` (two source lines merged
/// into one). For a multi-line delete that removes `N` newlines:
/// `removed = N + 1`, `inserted = 1` (`N + 1` pre-edit lines
/// collapsed into one post-edit line).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowSplice {
    /// First source-line index touched by the splice. Same in
    /// pre-edit and post-edit coordinates (lines before this point
    /// are unaffected).
    pub at: u32,
    /// Number of OLD source-line slots being removed at `at`.
    pub removed: u32,
    /// Number of NEW source-line slots being inserted at `at`.
    pub inserted: u32,
    /// Post-edit source-line indices that need their row counts
    /// recomputed after the splice is applied. Sorted, deduplicated.
    pub dirty: Vec<u32>,
}

impl RowSplice {
    /// Net change in the document's source-line count after the
    /// splice. Positive for inserts, negative for deletes.
    #[must_use]
    pub fn line_delta(&self) -> i64 {
        self.inserted as i64 - self.removed as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_index::IndexStamps;

    fn stamps() -> IndexStamps {
        IndexStamps::default()
    }

    #[test]
    fn set_row_count_updates_prefix_sum() {
        let mut index = DisplayRowIndex::from_row_counts(vec![1, 1, 1, 1], stamps());
        assert_eq!(index.display_row_count(), 4);
        index.set_row_count(SourceLine(2), 3);
        assert_eq!(index.display_row_count(), 6);
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(3)).raw(),
            5
        );
        assert_eq!(index.display_row_count_for_source(SourceLine(2)), 3);
    }

    #[test]
    fn splice_rows_in_place_updates_prefix_sum() {
        let mut index = DisplayRowIndex::from_row_counts(vec![1, 1, 1, 1], stamps());
        assert_eq!(index.display_row_count(), 4);
        let splice = RowSplice {
            at: 1,
            removed: 1,
            inserted: 2,
            dirty: vec![1, 2],
        };
        index.splice_rows(&splice, 1);
        assert_eq!(index.source_line_count(), 5);
        assert_eq!(index.display_row_count(), 5);
        // Lookup after splice should walk the new prefix tree.
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(4)).raw(),
            4,
        );
    }

    #[test]
    fn splice_rows_delete_collapses_slots() {
        let mut index = DisplayRowIndex::from_row_counts(vec![1, 2, 1, 1], stamps());
        assert_eq!(index.display_row_count(), 5);
        let splice = RowSplice {
            at: 1,
            removed: 2,
            inserted: 1,
            dirty: vec![1],
        };
        index.splice_rows(&splice, 1);
        assert_eq!(index.source_line_count(), 3);
        assert_eq!(index.display_row_count(), 3);
    }
}
