//! Paint-trace detail helpers for manual performance investigations.
//!
//! Thread ownership: UI thread of one window. These helpers only run
//! when `CONTINUITY_UI_TRACE` is enabled.

use std::ops::Range;

use continuity_render::{FrameDisplay, RenderStats};

/// Emit one aggregate projection-detail line for the focused paint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn log_projection_stats(
    frame_display: &FrameDisplay,
    viewport_rows: &Range<u32>,
    source_line_count: u32,
    frame_source: &'static str,
    build_kind: &'static str,
    worker_miss_reason: Option<&'static str>,
    selection_dirty_count: usize,
    image_reservation_count: usize,
    spectator_panes: usize,
    spectator_frame_displays: &[FrameDisplay],
    spectator_table_layout_count: usize,
    spectator_image_placement_count: usize,
    spectator_cache_hits: u32,
    spectator_cache_misses: u32,
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let realized = frame_display.realized_row_range();
    let realized_specs = frame_display.map().realized_display_lines().count();
    let spectator_realized_specs: usize = spectator_frame_displays
        .iter()
        .map(|frame| frame.map().realized_display_lines().count())
        .sum();
    let spectator_source_lines: u32 = spectator_frame_displays
        .iter()
        .map(|frame| frame.row_index().source_line_count())
        .sum();
    let detail = format!(
        concat!(
            "source_lines={} row_index_source_lines={} display_rows_total={} ",
            "viewport={}..{} realized={}..{} realized_specs={} ",
            "frame_source={} build_kind={} worker_miss={} ",
            "selection_dirty={} image_reservations={} spectator_panes={} ",
            "spectator_source_lines={} spectator_realized_specs={} ",
            "spectator_table_layouts={} spectator_images={} ",
            "spectator_cache_hits={} spectator_cache_misses={}"
        ),
        source_line_count,
        frame_display.row_index().source_line_count(),
        frame_display.display_line_count(),
        viewport_rows.start,
        viewport_rows.end,
        realized.start,
        realized.end,
        realized_specs,
        frame_source,
        build_kind,
        worker_miss_reason.unwrap_or("none"),
        selection_dirty_count,
        image_reservation_count,
        spectator_panes,
        spectator_source_lines,
        spectator_realized_specs,
        spectator_table_layout_count,
        spectator_image_placement_count,
        spectator_cache_hits,
        spectator_cache_misses,
    );
    crate::paint_trace::log_event("paint:projection_stats", &detail);
}

/// Emit one aggregate renderer-detail line for the focused paint.
pub(crate) fn log_render_stats(stats: &RenderStats, enclosing_draw_us: Option<u64>) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    crate::paint_trace::log_event("paint:render_stats", &stats.trace_detail());
    crate::paint_trace::log_event(
        "renderer_draw_stages",
        &stats
            .draw_stages_for_enclosing(enclosing_draw_us)
            .trace_detail(),
    );
    crate::paint_trace::log_event(
        "renderer_post_body_stages",
        &stats.post_body_stages.trace_detail(),
    );
    crate::paint_trace::log_event("chrome_path", &stats.chrome_path.trace_detail());
    crate::paint_trace::log_event("table_chrome_path", &stats.table_chrome_path.trace_detail());
}
