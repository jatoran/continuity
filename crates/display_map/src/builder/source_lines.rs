//! Source-line-scoped display-spec materialization for embedded viewports.

use crate::error::Error;
use crate::line::DisplayLineSpec;
use crate::wrap::WidthMeasure;

use super::DisplayMapBuilder;

impl DisplayMapBuilder<'_> {
    /// Materialize only a requested source-line range without walking the
    /// rest of the document. This is the storage-neutral projection path
    /// used by viewport-scoped embedded clients that do not need a global
    /// display-row index because wrapping is owned by the host browser.
    ///
    /// # Errors
    ///
    /// Returns the same validation and measurement errors as [`Self::build`].
    pub fn build_source_lines(
        self,
        source_lines: std::ops::Range<u32>,
        measure: &mut dyn WidthMeasure,
    ) -> Result<Vec<DisplayLineSpec>, Error> {
        self.validate_inputs()?;
        let rope = self.snapshot.rope();
        let start = source_lines.start.min(rope.len_lines() as u32);
        let end = source_lines.end.min(rope.len_lines() as u32).max(start);
        let mut lines = Vec::with_capacity(end.saturating_sub(start) as usize);
        let mut reservation_cursor = self
            .image_reservations
            .partition_point(|reservation| reservation.source_line.raw() < start);
        for source_line in start..end {
            self.materialize_source_line(
                rope,
                source_line,
                &mut lines,
                &mut reservation_cursor,
                measure,
            )?;
        }
        Ok(lines)
    }
}
