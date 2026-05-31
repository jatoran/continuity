//! Spectator-pane projection worker routing.
//!
//! Layout-template applies can remint pane ids before spectator panes
//! have any `SpectatorFrameCache` entries, a pane-focus change leaves
//! the just-defocused pane without a spectator-geometry cache entry
//! (its prior seed used focused-pane wrap_width), and a tree-sitter
//! decoration delivery shifts the `decoration_revision` field of the
//! spectator's cache key so a previously-valid entry now misses as
//! `Stale(decoration_revision)`. This module extends the layout-change,
//! focus-change, and decoration-change prewarms to non-focused panes
//! and drains their worker results back into the UI-thread spectator
//! cache.
//!
//! Thread ownership: UI thread of one window. The projection worker
//! builds `FrameDisplay`s off-thread; this module is the only path that
//! writes those worker-produced frames into
//! [`crate::window_spectator_cache::SpectatorFrameCache`].

use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_render::FrameDisplay;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::pane_tree::PaneId;
use crate::projection_worker::{ProjectionPlan, ProjectionStamp};
use crate::window::{Window, LINE_HEIGHT_DIP};
use crate::window_paint::{collect_non_focused_panes, visible_display_row_range};
use crate::window_paint_builders::NonFocusedPaneRender;
use crate::window_projection_worker::{current_projection_stamp, PaintProjectionInputs};

struct SpectatorProjectionContext {
    query: PrewarmQuery,
    stamp: ProjectionStamp,
    decorations: Option<Arc<Decorations>>,
    parse_revision: Option<u64>,
    caret_bytes: Vec<usize>,
    folds: Vec<FoldRange>,
    image_reservations: Vec<ImageRowReservation>,
    suppressed_table_blocks: Vec<std::ops::Range<usize>>,
}

struct SpectatorImageReservationInputs<'a> {
    decorations: Option<&'a Decorations>,
    rope: &'a ropey::Rope,
    pane: &'a NonFocusedPaneRender,
    revision: u64,
    caret_bytes: &'a [usize],
    suppressed_table_blocks: &'a [std::ops::Range<usize>],
    projection_char_width: f32,
}

impl Window {
    /// Submit layout-change projection prewarms for the focused pane and
    /// every spectator pane currently present in the live pane tree.
    pub(crate) fn try_dispatch_layout_projection_worker_for_live_panes(
        &mut self,
        reason: &'static str,
    ) {
        self.try_dispatch_projection_worker_for_live_panes(reason, "layout_change");
    }

    /// Submit focus-change projection prewarms for the focused pane and
    /// every spectator pane currently present in the live pane tree.
    /// Pane-focus change leaves the formerly-focused pane in spectator
    /// status without a spectator-geometry cache entry (its prior seed
    /// used focused-pane wrap_width). Without a prewarm the first
    /// post-focus spectator paint for that pane cold-walks the row
    /// index inline.
    pub(crate) fn try_dispatch_focus_change_projection_worker_for_live_panes(
        &mut self,
        reason: &'static str,
    ) {
        self.try_dispatch_projection_worker_for_live_panes(reason, "focus_change");
    }

    /// Submit decoration-change projection prewarms for every spectator
    /// pane whose buffer appears in `buffer_ids`. Tree-sitter's worker
    /// delivers fresh decorations asynchronously; the next paint after
    /// delivery sees the spectator cache as `Stale(decoration_revision)`
    /// because the cache key includes the decoration revision. Without
    /// a pending worker submission at the new revision the bounded
    /// partial fallback cannot fire and paint falls through to a 3 s
    /// row-count walker. The focused pane is intentionally not covered
    /// here — the existing focused-side dispatch in the paint pipeline
    /// owns it.
    pub(crate) fn try_dispatch_decoration_change_projection_worker_for_live_panes(
        &mut self,
        buffer_ids: &[u128],
    ) {
        if buffer_ids.is_empty() {
            return;
        }
        self.try_dispatch_spectator_projection_worker_for_live_panes(
            "decoration_delivered",
            "decoration_change",
            Some(buffer_ids),
        );
    }

    fn try_dispatch_projection_worker_for_live_panes(
        &mut self,
        reason: &'static str,
        submit_reason: &'static str,
    ) {
        let _ = self.try_dispatch_projection_worker_early(reason, submit_reason);
        self.try_dispatch_spectator_projection_worker_for_live_panes(reason, submit_reason, None);
    }

    /// Drain worker results that target non-focused panes and seed the
    /// spectator frame cache under each live pane id.
    pub(crate) fn drain_spectator_projection_worker_results(&self) {
        let Some(worker) = self.projection_worker.as_ref() else {
            return;
        };
        let live_panes = self.tree.root.leaf_ids();
        worker.retain_results_for_live_panes(&live_panes);
        let other_panes = collect_non_focused_panes(self);
        let mut populated = false;
        for pane in &other_panes {
            let Some(result) = worker.take_latest_result_for_target(pane.pane_id) else {
                continue;
            };
            let context = self.spectator_projection_context(pane);
            if result.stamp.diff_field(&context.stamp).is_some() {
                continue;
            }
            self.insert_spectator_worker_frame(
                pane.pane_id,
                pane.document,
                context,
                result.frame_display,
                result.build_dur_us,
            );
            populated = true;
        }
        if populated {
            self.invalidate_with_reason(self.hwnd, "spectator_cache_populate");
        }
    }

    /// `true` when a worker request for this spectator pane and stamp is
    /// queued or currently building.
    #[must_use]
    pub(crate) fn has_pending_spectator_projection(
        &self,
        pane_id: PaneId,
        stamp: &ProjectionStamp,
    ) -> bool {
        self.projection_worker
            .as_ref()
            .is_some_and(|worker| worker.has_pending_target_stamp(pane_id, stamp))
    }

    fn try_dispatch_spectator_projection_worker_for_live_panes(
        &mut self,
        reason: &'static str,
        submit_reason: &'static str,
        buffer_id_filter: Option<&[u128]>,
    ) {
        if self.projection_worker.is_none() || self.text_format.is_none() {
            return;
        }
        let other_panes = collect_non_focused_panes(self);
        if other_panes.is_empty() {
            return;
        }
        let projection_char_width = self.current_display_projection_metrics().char_width_dip;
        for pane in &other_panes {
            if let Some(filter) = buffer_id_filter {
                if !filter.contains(&pane.document) {
                    continue;
                }
            }
            let context = self.spectator_projection_context(pane);
            if self
                .projection_worker
                .as_ref()
                .is_some_and(|worker| worker.has_pending_target_stamp(pane.pane_id, &context.stamp))
            {
                continue;
            }
            let seq = self.next_projection_request_seq();
            let request = crate::window_projection_worker::build_projection_request(
                seq,
                pane.pane_id,
                context.stamp.clone(),
                pane.snapshot.rope_snapshot().rope(),
                context.decorations.clone(),
                &context.caret_bytes,
                &context.folds,
                &context.image_reservations,
                &context.suppressed_table_blocks,
                projection_char_width.max(1.0),
                ProjectionPlan::Cold,
            );
            if let Some(worker) = self.projection_worker.as_ref() {
                let submitted = worker.submit_with_reason(request, submit_reason);
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "event:projection_worker_early_dispatch",
                        &format!(
                            "reason={reason} target=spectator pane_id={:032x} \
                             submitted={submitted} plan=cold stamp_rev={} seq={seq}",
                            pane.pane_id.0 as u128, context.stamp.rope_revision,
                        ),
                    );
                }
            }
        }
    }

    fn spectator_projection_context(
        &self,
        pane: &NonFocusedPaneRender,
    ) -> SpectatorProjectionContext {
        let rope = pane.snapshot.rope_snapshot().rope();
        let revision = pane.snapshot.rope_snapshot().revision().0;
        let decorations = self.decoration_cache.get_arc(pane.document).cloned();
        let decoration_revision = decorations.as_ref().map(|d| d.revision);
        let caret_bytes = Self::caret_bytes_for_projection(rope, pane.snapshot.selections());
        let folds: Vec<FoldRange> = Vec::new();
        let projection_char_width = self.current_display_projection_metrics().char_width_dip;
        let suppressed_table_blocks =
            compute_spectator_suppressed_table_blocks(decorations.as_deref(), rope, pane);
        let image_reservations =
            self.compute_spectator_image_reservations(SpectatorImageReservationInputs {
                decorations: decorations.as_deref(),
                rope,
                pane,
                revision,
                caret_bytes: &caret_bytes,
                suppressed_table_blocks: &suppressed_table_blocks,
                projection_char_width: projection_char_width.max(1.0),
            });
        let wrap_width_dip = spectator_wrap_width_dip(self, pane);
        let viewport_h = pane.view.viewport_height_dip.max(pane.rect.3);
        let viewport_rows =
            visible_display_row_range(pane.view.scroll_y_dip, viewport_h, LINE_HEIGHT_DIP);
        // Deferred font-swap (see `window_font_swap`): spectator panes
        // share the window-wide font state, so a pending font commit
        // also drives their projection rebuilds against the target
        // font_state. Otherwise the spectator's display map would lag
        // a font swap by an extra round trip.
        let effective_font_state = self.effective_font_state();
        let query = PrewarmQuery::new(
            pane.buffer_id,
            revision,
            decoration_revision,
            &caret_bytes,
            &folds,
            wrap_width_dip,
            effective_font_state,
        )
        .with_image_reservations(&image_reservations);
        let stamp = current_projection_stamp(&PaintProjectionInputs {
            buffer_id: pane.buffer_id,
            rope_revision: revision,
            decoration_revision,
            decoration_parse_revision: decoration_revision,
            caret_bytes: &caret_bytes,
            folds: &folds,
            image_reservations: &image_reservations,
            wrap_width_dip,
            font_state: effective_font_state,
            viewport_rows: viewport_rows.clone(),
            overscan: crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
        });
        SpectatorProjectionContext {
            query,
            stamp,
            decorations,
            parse_revision: decoration_revision,
            caret_bytes,
            folds,
            image_reservations,
            suppressed_table_blocks,
        }
    }

    fn insert_spectator_worker_frame(
        &self,
        pane_id: PaneId,
        document: u128,
        context: SpectatorProjectionContext,
        frame_display: FrameDisplay,
        build_dur_us: u64,
    ) {
        let rope_revision = context.stamp.rope_revision;
        let decoration_revision = context
            .stamp
            .decoration_revision
            .map_or(-1i64, |revision| revision as i64);
        self.spectator_frame_cache.borrow_mut().insert(
            pane_id,
            context.query,
            frame_display,
            context.decorations,
            context.parse_revision,
        );
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "spectator_cache_populate",
                &format!(
                    "pane_id={:032x} document_id={document:032x} rope_rev={} \
                     decoration_rev={} source=worker_drain elapsed_us={build_dur_us}",
                    pane_id.0 as u128, rope_revision, decoration_revision,
                ),
            );
        }
    }
}

fn compute_spectator_suppressed_table_blocks(
    decorations: Option<&Decorations>,
    rope: &ropey::Rope,
    pane: &NonFocusedPaneRender,
) -> Vec<std::ops::Range<usize>> {
    let Some(decorations) = decorations else {
        return Vec::new();
    };
    // Spectator panes get their own suppression: Ctrl+A in pane A
    // must not unrender the same buffer's table in pane B.
    continuity_render::compute_suppressed_table_blocks(
        rope,
        pane.snapshot.selections(),
        &decorations.evaluated_tables,
    )
}

impl Window {
    fn compute_spectator_image_reservations(
        &self,
        inputs: SpectatorImageReservationInputs<'_>,
    ) -> Vec<ImageRowReservation> {
        let image_reservations = if let Some(renderer) = self.renderer.as_ref() {
            crate::window_image_placements::compute_image_reservations_for_pane(
                inputs.decorations,
                inputs.rope,
                self.image_store_dir.as_deref(),
                self.inline_images_enabled,
                inputs.pane.buffer_id,
                &self.image_expand_state,
                &mut |path| renderer.cached_image_dimensions(path),
                LINE_HEIGHT_DIP,
                inputs.pane.rect.2.max(1.0),
            )
        } else {
            Vec::new()
        };
        let table_layouts = compute_spectator_table_layouts(
            inputs.decorations,
            inputs.rope,
            inputs.revision,
            inputs.caret_bytes,
            inputs.suppressed_table_blocks,
            inputs.projection_char_width,
        );
        crate::window_image_placements::merge_table_row_reservations(
            image_reservations,
            &table_layouts,
        )
    }
}

fn compute_spectator_table_layouts(
    decorations: Option<&Decorations>,
    rope: &ropey::Rope,
    revision: u64,
    caret_bytes: &[usize],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    projection_char_width: f32,
) -> Vec<continuity_render::TableLayout> {
    let Some(decorations) = decorations else {
        return Vec::new();
    };
    if decorations.revision != revision || decorations.evaluated_tables.is_empty() {
        return Vec::new();
    }
    let mut measure = |text: &str| text.chars().count() as f32 * projection_char_width;
    continuity_render::compute_table_layouts(
        &decorations.evaluated_tables,
        rope,
        caret_bytes,
        suppressed_table_blocks,
        &mut measure,
    )
}

fn spectator_wrap_width_dip(window: &Window, pane: &NonFocusedPaneRender) -> u32 {
    if !pane.view.soft_wrap {
        return 0;
    }
    continuity_render::pane_body::spectator_body_text_width_with_right_edge_for_line_count_dip(
        pane.rect.2,
        window.scaled_font_size(),
        window.view_options.line_numbers,
        pane.snapshot.rope_snapshot().rope().len_lines(),
        pane.minimap,
        pane.show_outline_sidebar,
        window.view_options.outline_sidebar_width_dip,
    )
    .round()
    .max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn spectator_table_layout_helper_preserves_wrapped_row_reservations() {
        let long = "wordone wordtwo wordthree wordfour wordfive wordsix wordseven";
        let src = format!("| {long} |\n|---|\n| body |\n");
        let rope = Rope::from_str(&src);
        let decorations = Decorations::compute(&src, 7).expect("decorations");

        let layouts = compute_spectator_table_layouts(Some(&decorations), &rope, 7, &[4], &[], 8.0);
        let reservations = continuity_render::table_row_reservations(&layouts);

        assert!(
            reservations
                .iter()
                .any(|reservation| reservation.reserved_display_rows > 1),
            "wrapped table rows must contribute to spectator worker reservations"
        );
    }
}
