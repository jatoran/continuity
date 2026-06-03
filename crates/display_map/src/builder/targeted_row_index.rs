//! Targeted row-index refresh helpers.
//!
//! These helpers update a small set of source-line row counts from a
//! previous whole-document index. Input paths use them when an exact
//! row-index cache lookup misses after a local edit; they avoid the
//! whole-document row-count walker while preserving O(log n) row
//! lookup on the refreshed index.

use std::sync::Arc;

use crate::error::Error;
use crate::id::SourceLine;
use crate::row_index::DisplayRowIndex;
use crate::wrap::WidthMeasure;

use super::row_counts::row_count_for_source_line;
use super::DisplayMapBuilder;

impl<'a> DisplayMapBuilder<'a> {
    /// Refresh row counts for `source_lines` on top of `previous`.
    ///
    /// Returns `None` when the previous index has a different
    /// source-line shape than the current rope. That case requires a
    /// splice-aware path; callers that only need to avoid input-thread
    /// stalls should fall back to a conservative source-line floor.
    ///
    /// # Errors
    ///
    /// Same validation and measurement errors as [`Self::build_viewport`].
    pub fn refresh_row_index_source_lines(
        self,
        previous: &DisplayRowIndex,
        source_lines: &[u32],
        measure: &mut dyn WidthMeasure,
    ) -> Result<Option<Arc<DisplayRowIndex>>, Error> {
        self.validate_inputs()?;
        // P18.5 — refreshing a few lines on a partial index would leave
        // the unwalked-range placeholders in place; the resulting index
        // would lie to consumers about row counts outside the walked
        // window. Force the caller to upgrade to a full walk first.
        if previous.is_partial() {
            return Ok(None);
        }
        let rope = self.snapshot.rope();
        let source_line_count = rope.len_lines() as u32;
        if previous.source_line_count() != source_line_count {
            return Ok(None);
        }

        let mut index = previous.clone();
        for &source_line in source_lines {
            if source_line >= source_line_count {
                continue;
            }
            let cursor = image_reservation_cursor_for(self.image_reservations, source_line);
            let count = row_count_for_source_line(
                rope,
                self.decorations,
                self.caret_bytes,
                self.folds,
                self.image_reservations,
                self.suppressed_table_blocks,
                self.markdown_toggles,
                self.wrap,
                measure,
                self.row_count_cache,
                source_line,
                cursor,
                None,
            )?;
            index.set_row_count(SourceLine(source_line), count);
        }
        index.set_stamps(self.build_stamps());
        Ok(Some(Arc::new(index)))
    }
}

fn image_reservation_cursor_for(
    reservations: &[crate::image_row_reservation_provider::ImageRowReservation],
    source_line: u32,
) -> usize {
    match reservations.binary_search_by(|r| r.source_line.raw().cmp(&source_line)) {
        Ok(idx) => idx,
        Err(idx) => idx,
    }
}
