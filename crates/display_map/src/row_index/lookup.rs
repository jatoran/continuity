//! Projection queries for [`DisplayRowIndex`] — source↔display row
//! mapping and viewport-range lookups. Every method here is read-only
//! and runs in O(log n) over the parent index's Fenwick prefix tree.

use std::ops::Range;

use crate::id::{DisplayLine, SourceLine};

use super::DisplayRowIndex;

impl DisplayRowIndex {
    /// Number of display rows for `source_line`, or `0` if folded or out
    /// of range.
    #[must_use]
    pub fn display_row_count_for_source(&self, source_line: SourceLine) -> u32 {
        let i = source_line.as_usize();
        if i >= self.row_counts.len() {
            return 0;
        }
        u32::from(self.row_counts[i])
    }

    /// Index of the *first* display row for `source_line`. For folded
    /// source lines this returns the next visible row (the answer the
    /// legacy `source_to_display_line` vector handed out). For
    /// out-of-range source lines this returns the document's row count.
    #[must_use]
    pub fn first_display_row_of_source_line(&self, source_line: SourceLine) -> DisplayLine {
        let i = source_line.as_usize().min(self.row_counts.len());
        DisplayLine(self.prefix_sums.prefix_sum(i) as u32)
    }

    /// Inverse query: which source line owns display row `row`, and at
    /// what row offset within that source line?
    ///
    /// Returns `None` when `row` is past the document's last display
    /// row. Folded source lines (`row_count == 0`) are transparently
    /// skipped — the returned source line always has at least one
    /// display row.
    #[must_use]
    pub fn source_line_for_display_row(&self, row: u32) -> Option<(SourceLine, u32)> {
        self.prefix_sums
            .find_by_prefix(u64::from(row))
            .map(|(i, offset)| (SourceLine::from_usize(i), offset))
    }

    /// Source-line range whose display rows intersect `rows`.
    ///
    /// Returned as a half-open `[start..end)` source-line range. The
    /// builder uses this to decide which source lines to materialize
    /// when projecting only a viewport window: feed it
    /// `viewport_first_row..viewport_last_row` (already expanded with
    /// overscan) and materialize a `DisplayLineSpec` for each source
    /// line in the returned range. Folded source lines on the edges
    /// are excluded — only source lines that actually contribute at
    /// least one display row within `rows` appear.
    #[must_use]
    pub fn source_lines_for_display_rows(&self, rows: Range<u32>) -> Range<usize> {
        let n = self.row_counts.len();
        if n == 0 {
            return 0..0;
        }
        if rows.start >= rows.end {
            return 0..0;
        }
        let total = self.display_row_count();
        let clamped_start = rows.start.min(total);
        if clamped_start >= total {
            return n..n;
        }
        let start = self
            .prefix_sums
            .find_by_prefix(u64::from(clamped_start))
            .map_or(n, |(i, _)| i);
        // The end row is exclusive; we want the source line containing
        // the last *included* display row. Clamp into bounds so a
        // viewport that overruns the document still produces a valid
        // range.
        let last_row = rows.end.saturating_sub(1).min(total.saturating_sub(1));
        let end = self
            .prefix_sums
            .find_by_prefix(u64::from(last_row))
            .map_or(n, |(i, _)| i + 1);
        start..end.max(start)
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
    fn one_row_per_source_line_unfolded() {
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 1], stamps());
        assert_eq!(index.source_line_count(), 3);
        assert_eq!(index.display_row_count(), 3);
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(0)).raw(),
            0
        );
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(2)).raw(),
            2
        );
        assert_eq!(index.display_row_count_for_source(SourceLine(1)), 1);
    }

    #[test]
    fn folded_source_lines_have_zero_rows_and_share_next_visible_index() {
        let index = DisplayRowIndex::from_row_counts(vec![1, 0, 0, 1], stamps());
        assert_eq!(index.display_row_count(), 2);
        assert_eq!(index.display_row_count_for_source(SourceLine(1)), 0);
        // Folded lines collapse onto the next visible display row.
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(1)).raw(),
            1
        );
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(2)).raw(),
            1
        );
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(3)).raw(),
            1
        );
    }

    #[test]
    fn out_of_range_source_line_points_past_eof() {
        let index = DisplayRowIndex::from_row_counts(vec![1, 1], stamps());
        // Source line 5 doesn't exist; the index clamps to the document.
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(5)).raw(),
            2
        );
        assert_eq!(index.display_row_count_for_source(SourceLine(5)), 0);
    }

    #[test]
    fn soft_wrapped_source_lines_have_multiple_rows() {
        // Source line 0 wraps into 3 rows; line 1 is unwrapped.
        let index = DisplayRowIndex::from_row_counts(vec![3, 1], stamps());
        assert_eq!(index.display_row_count(), 4);
        assert_eq!(
            index.first_display_row_of_source_line(SourceLine(1)).raw(),
            3
        );
        // Reverse lookups land on the right source line + row offset.
        assert_eq!(
            index.source_line_for_display_row(0),
            Some((SourceLine(0), 0))
        );
        assert_eq!(
            index.source_line_for_display_row(2),
            Some((SourceLine(0), 2))
        );
        assert_eq!(
            index.source_line_for_display_row(3),
            Some((SourceLine(1), 0))
        );
        assert_eq!(index.source_line_for_display_row(4), None);
    }

    #[test]
    fn source_lines_for_display_rows_clips_to_viewport() {
        // Row counts: [1, 1, 3, 1, 1] — prefix sums 0,1,2,5,6,7.
        let index = DisplayRowIndex::from_row_counts(vec![1, 1, 3, 1, 1], stamps());
        assert_eq!(index.display_row_count(), 7);
        // Pure interior viewport.
        assert_eq!(index.source_lines_for_display_rows(2..5), 2..3);
        // Viewport spanning multiple source lines.
        assert_eq!(index.source_lines_for_display_rows(1..6), 1..4);
        // Empty range yields empty.
        assert_eq!(index.source_lines_for_display_rows(3..3), 0..0);
        // Overrun the document: clamps into bounds.
        assert_eq!(index.source_lines_for_display_rows(0..1000), 0..5);
    }

    #[test]
    fn source_lines_for_display_rows_skips_folded_edges() {
        // Folded slots at positions 1 and 3; the rest have one row.
        let index = DisplayRowIndex::from_row_counts(vec![1, 0, 1, 0, 1], stamps());
        // Row 0 → source 0; row 1 → source 2 (skip folded 1); row 2 →
        // source 4 (skip folded 3).
        assert_eq!(index.source_lines_for_display_rows(0..1), 0..1);
        assert_eq!(index.source_lines_for_display_rows(1..2), 2..3);
        assert_eq!(index.source_lines_for_display_rows(0..3), 0..5);
    }

    #[test]
    fn source_lines_for_display_rows_empty_index() {
        let index = DisplayRowIndex::from_row_counts(vec![], stamps());
        assert_eq!(index.source_lines_for_display_rows(0..10), 0..0);
    }
}
