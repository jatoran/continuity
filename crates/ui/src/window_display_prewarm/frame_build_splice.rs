//! Cross-revision row-index splice fast-path.
//!
//! Sibling of [`super::frame_build`]. When the cross-pane row-index
//! cache misses on `rope_revision` but holds a same-document entry at
//! the same `(wrap_width_dip, font_state, fold_signature)`, the splice
//! path fetches the byte-delta chain between the cached entry's
//! revision and the current revision from
//! [`continuity_core::EditorHandle::rope_deltas_since`], then asks the
//! display-map builder to splice the older index forward in place
//! (single-line refresh on within-line edits, structural splice on
//! line-count changes). On success the spliced index is cached under
//! the live key and the cold row-count walker is skipped entirely.
//!
//! When the delta chain is broken (history evicted past the cached
//! revision), the classifier returns `RowDirty::FullRebuild`, or the
//! line counts don't add up to the live rope, the helper returns
//! `None` so the caller falls back to a cold walker run.
//!
//! Runs on the [`crate::Window`]-owning UI thread.

use std::sync::Arc;

use continuity_buffer::BufferId;
use continuity_decorate::Decorations;
use continuity_display_map::{
    DisplayRowIndex, FoldRange, ImageRowReservation, RowIndexSpliceStats,
};
use continuity_render::{DirectWriteWidthMeasure, FrameDisplay};

use crate::window::Window;
use crate::window_row_index_cache::RowIndexKey;

/// Outcome of a splice attempt.
pub(crate) struct SplicedRowIndex {
    pub row_index: Arc<DisplayRowIndex>,
    pub stats: RowIndexSpliceStats,
    pub prev_rope_revision: u64,
}

impl Window {
    /// Attempt to splice the cross-pane row-index cache's most-recent
    /// same-document entry forward to `live_key.rope_revision`.
    ///
    /// Returns `None` when no candidate exists, the delta chain is
    /// broken, or the classifier required a full rebuild. The caller
    /// falls back to a cold walker run in that case.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_splice_row_index_forward(
        &self,
        live_key: &RowIndexKey,
        buffer_id: BufferId,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
    ) -> Option<SplicedRowIndex> {
        let previous = self.row_index_cache.borrow().get_for_splice(live_key)?;
        let prev_rope_revision = previous.stamps().rope_revision;
        // Splice forward only. A "future" cached revision relative to
        // this paint's revision would require reversing deltas, which
        // the delta history doesn't supply; fall through to the cold
        // walker in that case.
        if prev_rope_revision >= revision {
            return None;
        }
        let (deltas, covered) = self.editor.rope_deltas_since(buffer_id, prev_rope_revision);
        if !covered {
            return None;
        }
        let (row_index, stats) = if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new_with_run_cache(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
                Some(Arc::clone(&self.walker_run_cache)),
                self.font_state,
                crate::window::FONT_LOCALE,
            );
            FrameDisplay::splice_row_index_forward_measured_with_caches(
                &previous,
                &deltas,
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                wrap_width_dip,
                &mut measure,
                self.font_state.0,
                crate::window::FONT_LOCALE,
                &self.walker_wrap_cache,
                &self.walker_segment_cache,
            )?
        } else {
            let mut measure =
                continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
            FrameDisplay::splice_row_index_forward_measured_with_caches(
                &previous,
                &deltas,
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                wrap_width_dip,
                &mut measure,
                self.font_state.0,
                crate::window::FONT_LOCALE,
                &self.walker_wrap_cache,
                &self.walker_segment_cache,
            )?
        };
        Some(SplicedRowIndex {
            row_index,
            stats,
            prev_rope_revision,
        })
    }
}
