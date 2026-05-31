//! FrameDisplay resolution for mouse hit-testing.

use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_display_map::{DisplayRowIndex, FoldRange, FoldSignature, IndexStamps};
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::LINE_HEIGHT_DIP;
use crate::window_mouse_hit_test_cache::MouseHitTestFrameCacheEntry;
use crate::window_paint::{visible_display_row_range, VIEWPORT_OVERSCAN_ROWS};
use crate::Window;

impl Window {
    pub(crate) fn hit_test_projection_query_and_folds(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        wrap_width_dip: u32,
    ) -> (PrewarmQuery, Vec<FoldRange>) {
        let heading_lines =
            self.cached_heading_lines_for_projection(self.buffer_id, rope, revision, decorations);
        let folds = self.display_projection_folds(rope, &heading_lines, caret_bytes);
        let query = PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|decorations| decorations.revision),
            caret_bytes,
            &folds,
            wrap_width_dip,
            self.font_state,
        );
        (query, folds)
    }

    /// Resolve a [`FrameDisplay`] suitable for hit-testing the focused
    /// pane. Hit-tests map to what the user saw, so painted and
    /// spectator frames are preferred over rebuilding at the newest
    /// rope/decorations revision.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolve_hit_test_frame_display(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        wrap_width_dip: u32,
        column_advance: f32,
        target_display_row: u32,
    ) -> (FrameDisplay, &'static str, Vec<FoldRange>) {
        let (hit_test_query, folds) = self.hit_test_projection_query_and_folds(
            rope,
            revision,
            decorations,
            caret_bytes,
            wrap_width_dip,
        );
        if let Some((cached_query, fd)) = self.last_painted_frame_display.as_ref() {
            if cached_query.is_compatible_for_hit_test(&hit_test_query) {
                if fd.realized_row_range().contains(&target_display_row) {
                    if crate::paint_trace::is_trace_enabled() {
                        crate::paint_trace::log_event(
                            "click_hit_test_frame_source",
                            "source=last_painted",
                        );
                    }
                    return (fd.clone(), "last_painted", folds);
                }
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "click_hit_test_frame_source",
                        "source=fallback stale_cache=realized_range",
                    );
                }
            } else if crate::paint_trace::is_trace_enabled() {
                let mismatch = cached_query
                    .hit_test_compat_mismatch(&hit_test_query)
                    .unwrap_or("unknown");
                crate::paint_trace::log_event(
                    "click_hit_test_frame_source",
                    &format!("source=fallback stale_cache={mismatch}"),
                );
            }
        } else if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event("click_hit_test_frame_source", "source=cache_empty");
        }

        let focused_pane = self.tree.focused;
        match self
            .spectator_frame_cache
            .borrow()
            .lookup_for_hit_test_with_reason(focused_pane, &hit_test_query)
        {
            Ok(fd) => {
                if fd.realized_row_range().contains(&target_display_row) {
                    if crate::paint_trace::is_trace_enabled() {
                        crate::paint_trace::log_event(
                            "click_hit_test_frame_source",
                            "source=spectator_cache",
                        );
                    }
                    return (fd, "spectator_cache", folds);
                }
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "spectator_cache_lookup",
                        "path=hit_test result=miss miss_reason=realized_range",
                    );
                }
            }
            Err(reason) => {
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "spectator_cache_lookup",
                        &format!("path=hit_test result=miss miss_reason={reason}"),
                    );
                }
            }
        }

        if let Some(fd) = self
            .spectator_frame_cache
            .borrow()
            .lookup_same_document(focused_pane, &hit_test_query)
        {
            if fd.realized_row_range().contains(&target_display_row) {
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "click_hit_test_frame_source",
                        "source=spectator_cache_wrap_tolerant",
                    );
                }
                return (fd, "spectator_cache_wrap_tolerant", folds);
            }
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "spectator_cache_lookup",
                    "path=wrap_tolerant result=miss miss_reason=realized_range",
                );
            }
        } else if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "spectator_cache_lookup",
                "path=wrap_tolerant result=miss miss_reason=no_same_document_entry",
            );
        }

        if let Some(entry) = self.mouse_hit_test_frame_cache.borrow().as_ref() {
            if entry.query().is_compatible_for_hit_test(&hit_test_query)
                && entry
                    .frame_display()
                    .realized_row_range()
                    .contains(&target_display_row)
            {
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "click_hit_test_frame_source",
                        "source=mouse_cache",
                    );
                }
                return (entry.frame_display().clone(), "mouse_cache", folds);
            }
        }

        let visible_rows = visible_display_row_range(
            self.view.scroll_y_dip,
            self.view.viewport_height_dip,
            LINE_HEIGHT_DIP,
        );
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "click_hit_test_frame_source",
                &format!(
                    "source=viewport_build visible={}..{} source_lines={}",
                    visible_rows.start,
                    visible_rows.end,
                    rope.len_lines(),
                ),
            );
        }
        let fd = if self.has_cached_row_index_for_frame_display_viewport(
            Some(self.buffer_id),
            revision,
            decorations,
            &folds,
            &[],
            wrap_width_dip,
        ) {
            self.build_frame_display_viewport_cached(
                Some(self.buffer_id),
                rope,
                revision,
                decorations,
                caret_bytes,
                &folds,
                &[],
                wrap_width_dip,
                column_advance,
                visible_rows,
                VIEWPORT_OVERSCAN_ROWS,
                continuity_display_map::WalkerCallReason::ViewportRealize,
            )
        } else {
            self.build_mouse_hit_test_source_floor_frame_display(
                rope,
                revision,
                decorations,
                caret_bytes,
                &folds,
                column_advance,
                visible_rows,
            )
        };
        let decorations_owned = decorations.cloned().map(Arc::new);
        *self.mouse_hit_test_frame_cache.borrow_mut() = Some(MouseHitTestFrameCacheEntry::new(
            hit_test_query.clone(),
            fd.clone(),
            decorations_owned,
            decorations.map(|decorations| decorations.revision),
        ));
        (fd, "viewport_build", folds)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_mouse_hit_test_source_floor_frame_display(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        column_advance: f32,
        visible_rows: std::ops::Range<u32>,
    ) -> FrameDisplay {
        let row_count = rope.len_lines().max(1);
        let row_index = Arc::new(DisplayRowIndex::from_row_counts(
            vec![1u16; row_count],
            IndexStamps {
                rope_revision: revision,
                decoration_revision: decorations.map_or(revision, |d| d.revision),
                wrap_width_dip: 0,
                font_state: self.font_state.0,
                fold_signature: FoldSignature::compute(folds),
            },
        ));
        let mut measure =
            continuity_display_map::wrap::FixedCharWidth::new(column_advance.max(1.0));
        FrameDisplay::build_viewport_with_row_index_measured(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            &[],
            0,
            &mut measure,
            visible_rows,
            VIEWPORT_OVERSCAN_ROWS,
            row_index,
        )
    }
}
