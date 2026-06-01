//! D2D draw dispatch + projection-worker request submission for the
//! `Window::on_paint` pipeline.
//!
//! `dispatch_renderer_draw` owns the conditional fork between the
//! normal body paint and custom overlay surfaces (metrics buffer,
//! buffer-history tab). Call ordering of `renderer.draw_buffer*` and
//! the overlay helper follow-ups is preserved exactly —
//! perf scripts in `.trash/analyze_trace.py` consume `trace.mark`
//! labels in this order. The overlay paints themselves stay on
//! `Window` and are invoked by the caller *after* `dispatch_renderer_draw`
//! returns, so the `&DrawParams` borrows have ended by then.
//!
//! `submit_projection_worker_request` fire-and-forgets the next worker
//! plan after the renderer borrow has released. Per-paint image-row
//! reservations ride through the worker request and stamp, so worker
//! results are accepted only for the exact reservation geometry paint
//! asked for.

use std::ops::Range;
use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, ImageRowReservation};
use continuity_layout::LayoutCache;
use continuity_render::{DrawParams, RenderStats, Renderer};
use ropey::Rope;

use crate::paint_trace::PaintTrace;
use crate::pane_layout::Rect;
use crate::projection_worker::{ProjectionPlan, ProjectionStamp, PAINT_PARTIAL_FILL_REASON};
use crate::window::Window;
use crate::window_projection_plan::ProjectionBuildKind;
use crate::Error;

pub(crate) fn should_skip_projection_worker_request_for_frame_source(frame_source: &str) -> bool {
    matches!(frame_source, "cache_hit" | "worker_hit")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_renderer_draw(
    cache: &mut LayoutCache,
    renderer: &Renderer,
    snap_rope: &Rope,
    snap_selections: &[continuity_text::Selection],
    params: &DrawParams<'_>,
    metrics_overlay: bool,
    history_overlay: bool,
    trace: &PaintTrace,
) -> Result<(), Error> {
    let layout_counters_before = crate::paint_trace::is_trace_enabled().then(|| cache.counters());
    let mut render_stats = crate::paint_trace::is_trace_enabled()
        .then(|| RenderStats::from_draw_params(snap_rope, params));
    let draw_started = crate::paint_trace::is_trace_enabled().then(std::time::Instant::now);
    if metrics_overlay || history_overlay {
        // §I2 / buffer-history: paint regular chrome (tab strip,
        // status bar, pane borders) and body backdrops; the caller
        // then overlays custom panels and presents once.
        renderer.draw_buffer_no_present(snap_rope, snap_selections, cache, params)?;
    } else {
        renderer.draw_buffer(snap_rope, snap_selections, cache, params)?;
    }
    let enclosing_draw_us = draw_started
        .map(|started| u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
    if let (Some(stats), Some(before)) = (render_stats.as_mut(), layout_counters_before) {
        stats.add_layout_cache_delta(cache.counters().saturating_delta(before));
        stats.body_paint_us = renderer.last_body_paint_us();
        stats.post_body_paint_us = renderer.last_post_body_paint_us();
        stats.post_body_stages = renderer.last_post_body_stages();
        stats.chrome_path = renderer.last_chrome_path_stats();
        stats.table_chrome_path = renderer.last_table_chrome_stats();
        stats.chrome_overlay_breakdown = renderer.last_chrome_overlay_breakdown();
        crate::window_paint_trace::log_render_stats(stats, enclosing_draw_us);
        let rows_placeholder = renderer.last_scroll_placeholder_rows();
        let rows_realized_synchronously = renderer.last_scroll_strip_rows();
        let scroll_mode = if rows_placeholder > 0 {
            "fractional_placeholder"
        } else if rows_realized_synchronously > 0 {
            "fractional_realized"
        } else if params.view.scroll_y_dip.fract().abs() > f32::EPSILON
            || params.scroll_velocity_dip_per_s.abs() > f32::EPSILON
        {
            "fractional_only"
        } else {
            "cold"
        };
        // The cached/reused frame's scroll_y can be approximated from
        // its realized row range — `realized.start * line_height` is
        // the top of the strip the frame would paint when it was built.
        // Used by the perf analyzer to show the gap between live and
        // frame scroll positions during inertia.
        let realized = params.frame_display.realized_row_range();
        let frame_scroll_y_dip = realized.start as f32 * params.line_height.max(1.0);
        crate::paint_trace::log_event(
            "scroll_path",
            &format!(
                "mode={scroll_mode} elapsed_us={} velocity_dip_per_s={:.2} \
                 rows_realized_synchronously={rows_realized_synchronously} \
                 rows_placeholder={rows_placeholder} \
                 scroll_y_dip={:.2} frame_scroll_y_dip={frame_scroll_y_dip:.2} \
                 target_pane_id={:032x} focused_pane_id={:032x} hover_routed={}",
                enclosing_draw_us.unwrap_or(0),
                params.scroll_velocity_dip_per_s,
                params.view.scroll_y_dip,
                params.scroll_target_pane_id,
                params.scroll_focused_pane_id,
                params.scroll_hover_routed,
            ),
        );
        // Emit cumulative cache state (size + capacity + lifetime
        // hit/miss/created) alongside the per-paint delta in
        // `paint:render_stats`. The running summary aggregates
        // `event:layout_cache_state` over time so a long session has
        // total cache turnover on disk.
        let counters = cache.counters();
        crate::paint_trace::log_event(
            "layout_cache_state",
            &format!(
                "size={} capacity={} hits={} misses={} layouts_created={}",
                cache.len(),
                cache.capacity(),
                counters.hits,
                counters.misses,
                counters.layouts_created,
            ),
        );
    }
    trace.mark("renderer.draw_buffer");
    Ok(())
}

impl Window {
    /// Paint active custom overlay panels after `dispatch_renderer_draw`
    /// has finished and the `&DrawParams` self-sub-field borrows have
    /// ended, then present the combined frame.
    pub(crate) fn paint_overlay_after_dispatch(
        &mut self,
        metrics_overlay: bool,
        history_overlay: bool,
        body_rect: Rect,
    ) -> Result<(), Error> {
        if !metrics_overlay && !history_overlay {
            return Ok(());
        }
        if metrics_overlay {
            let overlay_rect = continuity_render::metrics_panel::PanelRect {
                left: body_rect.x,
                top: body_rect.y,
                right: body_rect.x + body_rect.w.max(1.0),
                bottom: body_rect.y + body_rect.h.max(1.0),
            };
            self.paint_metrics_buffer_overlay_no_present(overlay_rect)?;
        }
        if history_overlay {
            self.paint_visible_buffer_history_overlays_no_present()?;
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.present().map_err(Error::Render)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit_projection_worker_request(
        &mut self,
        projection_kind: &ProjectionBuildKind,
        frame_source: &'static str,
        current_projection_stamp: &ProjectionStamp,
        rope_for_projection: &Rope,
        decorations_owned: Option<Arc<Decorations>>,
        caret_bytes_for_projection: &[usize],
        folds_for_projection: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        projection_char_width: f32,
        viewport_rows: &Range<u32>,
    ) {
        let should_fill_partial_cache_hit = matches!(
            projection_kind,
            ProjectionBuildKind::CacheHit(frame) if frame.row_index().is_partial()
        );
        if should_skip_projection_worker_request_for_frame_source(frame_source)
            && !should_fill_partial_cache_hit
        {
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "event:projection_worker_dispatch",
                    &format!(
                        "source={frame_source} plan=none submitted=false \
                         reason=served_from_cache"
                    ),
                );
            }
            return;
        }
        // ε.5c — submit the same plan classification the inline
        // path would have built (Dirty/Splice/Cold). Cache-hit
        // kinds map to `None` and skip dispatch outright, except a
        // partial cache hit still submits a Cold fill so the worker can
        // replace the placeholder row counts with a full index.
        let plan_for_worker = if should_fill_partial_cache_hit {
            Some(ProjectionPlan::Cold)
        } else {
            projection_kind.to_worker_plan()
        };
        if let Some(plan) = plan_for_worker {
            let plan_label = crate::window_projection_plan::worker_plan_label(&plan);
            let submission_reason =
                if projection_kind.is_partial_variant() || should_fill_partial_cache_hit {
                    PAINT_PARTIAL_FILL_REASON
                } else {
                    "paint_epilogue"
                };
            if projection_kind.is_partial_variant()
                && self.projection_worker.as_ref().is_some_and(|worker| {
                    worker.has_pending_partial_fill_same_or_older_stamp(current_projection_stamp)
                })
            {
                if crate::paint_trace::is_trace_enabled() {
                    let detail = format!(
                        "plan={plan_label} reason={submission_reason} submitted=false \
                         dedupe_reason=pending_same_or_older_stamp viewport={}..{}",
                        viewport_rows.start, viewport_rows.end,
                    );
                    crate::paint_trace::log_event("event:projection_worker_dispatch", &detail);
                }
                return;
            }
            if self.projection_worker.as_ref().is_some_and(|worker| {
                worker.has_pending_target_stamp(self.tree.focused, current_projection_stamp)
            }) {
                if crate::paint_trace::is_trace_enabled() {
                    let detail = format!(
                        "plan={plan_label} viewport={}..{} submitted=false \
                         reason=pending_duplicate",
                        viewport_rows.start, viewport_rows.end,
                    );
                    crate::paint_trace::log_event("event:projection_worker_dispatch", &detail);
                }
                return;
            }
            let seq = self.next_projection_request_seq();
            // P18.5b/P18.6b — tag partial background-fill requests so
            // they can be traced and deduped independently of regular
            // paint epilogue and edit-driven early dispatch work.
            if let Some(worker) = self.projection_worker.as_ref() {
                let suppressed_table_blocks = self.compute_suppressed_table_blocks();
                let request = crate::window_projection_worker::build_projection_request(
                    seq,
                    self.tree.focused,
                    current_projection_stamp.clone(),
                    rope_for_projection,
                    decorations_owned,
                    caret_bytes_for_projection,
                    folds_for_projection,
                    image_reservations,
                    &suppressed_table_blocks,
                    projection_char_width,
                    self.projection_font_metrics(),
                    plan,
                );
                let submitted = worker.submit_with_reason(request, submission_reason);
                if crate::paint_trace::is_trace_enabled() {
                    let detail = format!(
                        "seq={seq} plan={plan_label} reason={submission_reason} \
                         viewport={}..{} submitted={submitted}",
                        viewport_rows.start, viewport_rows.end,
                    );
                    crate::paint_trace::log_event("event:projection_worker_dispatch", &detail);
                }
            }
        } else if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "event:projection_worker_dispatch",
                "plan=cache_hit submitted=false",
            );
        }
    }
}
