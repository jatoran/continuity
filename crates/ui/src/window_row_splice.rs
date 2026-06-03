//! ε.3F — paint-time integration for the row-index splice path.
//!
//! When [`continuity_display_map::DisplayRowIndex::dirty_after_rope_edits`]
//! returns [`continuity_display_map::RowDirty::Splice`], the paint
//! cache can incrementally update the row-index shape (instead of
//! cold-rebuilding the viewport). This module owns the
//! [`Window::rebuild_frame_display_spliced`] wrapper that pipes the
//! splice through `FrameDisplay::rebuild_spliced_measured`, plus the
//! [`log_row_index_splice`] trace emitter.
//!
//! Kept in a sibling file so `window_paint.rs` and
//! `window_display_prewarm.rs` stay near the conventions cap.
//!
//! Thread ownership: UI thread of one window.

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation, RowSplice};
use continuity_render::{DirectWriteWidthMeasure, FrameDisplay};
use continuity_text::RopeEditDelta;

use crate::window::Window;

impl Window {
    /// Apply a [`RowSplice`] to `prev` and realize the post-splice
    /// viewport. Mirrors [`Window::rebuild_frame_display_dirty`] but
    /// routes through the spliced builder path. Used by `on_paint`
    /// when a line-count edit fired the splice detection in
    /// `dirty_after_rope_edits`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rebuild_frame_display_spliced(
        &self,
        prev: &FrameDisplay,
        splice: &RowSplice,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        suppressed_table_blocks: &[std::ops::Range<usize>],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
    ) -> FrameDisplay {
        if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
            );
            return FrameDisplay::rebuild_spliced_measured(
                prev,
                splice,
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                suppressed_table_blocks,
                self.markdown_render_toggles(),
                wrap_width_dip,
                &mut measure,
                visible_rows,
                overscan,
            );
        }
        let mut measure =
            continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
        FrameDisplay::rebuild_spliced_measured(
            prev,
            splice,
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            suppressed_table_blocks,
            self.markdown_render_toggles(),
            wrap_width_dip,
            &mut measure,
            visible_rows,
            overscan,
        )
    }
}

/// Emit the `paint:frame_display:row_index_splice` trace event. Kept
/// here so `window_paint.rs` only carries the dispatch call site.
pub(crate) fn log_row_index_splice(
    splice: &RowSplice,
    deltas: &[RopeEditDelta],
    old_lines: u32,
    new_lines: u32,
    viewport_rows: &std::ops::Range<u32>,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let (affected_start, affected_end) = affected_byte_range(deltas);
    let (lines_added, lines_removed) = line_delta_parts(splice.line_delta());
    let edit_kind = splice_edit_kind(splice, deltas);
    let detail = format!(
        "edit_kind={edit_kind} lines_added={lines_added} lines_removed={lines_removed} \
         affected_byte_range={affected_start}..{affected_end} at={} removed={} inserted={} \
         line_delta={} dirty_count={} old_lines={old_lines} new_lines={new_lines} viewport={}..{}",
        splice.at,
        splice.removed,
        splice.inserted,
        splice.line_delta(),
        splice.dirty.len(),
        viewport_rows.start,
        viewport_rows.end,
    );
    crate::paint_trace::log_event("paint:frame_display:row_index_splice", &detail);
}

fn affected_byte_range(deltas: &[RopeEditDelta]) -> (u32, u32) {
    let Some(start) = deltas.iter().map(|delta| delta.at).min() else {
        return (0, 0);
    };
    let end = deltas
        .iter()
        .map(|delta| {
            delta
                .at
                .saturating_add(delta.removed_bytes.max(delta.inserted_bytes))
        })
        .max()
        .unwrap_or(start);
    (clamp_usize_to_u32(start), clamp_usize_to_u32(end))
}

fn clamp_usize_to_u32(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

fn line_delta_parts(line_delta: i64) -> (i64, i64) {
    if line_delta >= 0 {
        (line_delta, 0)
    } else {
        (0, -line_delta)
    }
}

fn splice_edit_kind(splice: &RowSplice, deltas: &[RopeEditDelta]) -> &'static str {
    let has_insert = deltas.iter().any(|delta| delta.inserted_bytes > 0);
    let has_delete = deltas.iter().any(|delta| delta.removed_bytes > 0);
    match (has_insert, has_delete, splice.line_delta().cmp(&0)) {
        (true, false, std::cmp::Ordering::Greater) => "insert_multiline",
        (false, true, std::cmp::Ordering::Less) => "delete_multiline",
        (true, true, _) => "replace",
        (true, false, _) => "insert",
        (false, true, _) => "delete",
        (false, false, _) => "unknown",
    }
}
