//! Viewport-row geometry helpers shared by `window_paint`,
//! `window_caret_anchor`, `window_mouse_hit_test`, `window_scroll`, the
//! projection-plan realizers, and the spectator-pane paint pass. Lives
//! alongside `window_paint.rs` so the orchestrator stays under the
//! 600-line cap.

/// ε.2 — extra display rows realized above and below the visible
/// viewport. Matches CodeMirror 6's "buffer above and below" pattern.
/// Keeps caret-line-shift refinement, hit-testing on the viewport
/// edges, and the start-of-scroll fast path materialized rows ready
/// to paint without another rebuild.
pub(crate) const VIEWPORT_OVERSCAN_ROWS: u32 = 20;

/// Compute the absolute display-row range the painter will iterate for
/// the current scroll/viewport state. Matches the per-row walk in
/// `crates/render/src/renderer.rs` and `wrap_paint.rs` so the
/// `build_viewport` realization covers every row the renderer paints.
pub(crate) fn visible_display_row_range(
    scroll_y_dip: f32,
    viewport_height_dip: f32,
    line_height_dip: f32,
) -> std::ops::Range<u32> {
    let line_h = line_height_dip.max(1.0);
    let scroll_y = scroll_y_dip.max(0.0);
    let viewport_h = viewport_height_dip.max(0.0);
    let first = (scroll_y / line_h).floor().max(0.0) as u32;
    let last_f = ((scroll_y + viewport_h) / line_h).ceil() + 1.0;
    let last = last_f.max(0.0) as u32;
    first..last.max(first.saturating_add(1))
}
