//! Caret-placement row-index refresh path.
//!
//! Owned by one [`crate::Window`] on the UI thread. The helper patches
//! a previous whole-document row index for the source lines whose row
//! counts can change after a caret edit, then materializes only the
//! caret source line's display-row range.

use continuity_decorate::Decorations;
use continuity_display_map::SourceLine;
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::Window;

impl Window {
    /// Build a caret-focused frame from a targeted row-index refresh.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_caret_anchor_targeted_frame_display(
        &self,
        query: &PrewarmQuery,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        source_line: usize,
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) -> Option<FrameDisplay> {
        let (previous_query, previous_frame) = self.last_painted_frame_display.as_ref()?;
        if previous_query.document() != query.document() {
            return None;
        }

        let mut source_lines = Vec::with_capacity(previous_query.caret_bytes().len() + 1);
        source_lines.push(source_line as u32);
        for byte in previous_query.caret_bytes() {
            let source_byte = (*byte).min(rope.len_bytes());
            source_lines.push(rope.byte_to_line(source_byte) as u32);
        }
        source_lines.sort_unstable();
        source_lines.dedup();

        let row_index = self.refresh_frame_display_row_index_source_lines(
            previous_frame.row_index(),
            &source_lines,
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            &[],
            wrap_width_dip,
            char_width_dip,
        )?;
        let source_line = SourceLine::from_usize(source_line);
        let display_row = row_index
            .first_display_row_of_source_line(source_line)
            .raw();
        let row_count = row_index.display_row_count_for_source(source_line);
        let target_rows = display_row..display_row.saturating_add(row_count.max(1));

        Some(self.build_frame_display_from_row_index(
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            &[],
            wrap_width_dip,
            char_width_dip,
            target_rows,
            0,
            row_index,
        ))
    }
}
