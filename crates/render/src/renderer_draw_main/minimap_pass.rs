//! Scaled-text minimap paint pass, lifted out of
//! [`super::render_frame`] so `renderer_draw_main.rs` stays under the
//! 600-line conventions cap.
//!
//! §28 — the minimap geometry is driven off the editor's display-row
//! content height (the same value the scrollbar uses) so the strip's
//! indicator and click resolution stay consistent with the editor
//! scroll under soft-wrap / folds / reserved rows.
//!
//! Thread ownership: UI thread (caller owns the device context).

use ropey::Rope;

use crate::params::DrawParams;
use crate::renderer::Renderer;

/// Paint the scaled-text minimap strip for the focused pane, returning
/// the elapsed microseconds for the `minimap_us` breakdown bucket. A
/// `0` return means the minimap toggle was off this frame.
pub(super) fn paint_minimap_pass(
    renderer: &Renderer,
    rope: &Rope,
    params: &DrawParams<'_>,
    viewport_w: f32,
    viewport_h: f32,
    line_height: f32,
    scroll_y: f32,
) -> u64 {
    if !params.view_options.minimap {
        return 0;
    }
    let started = std::time::Instant::now();
    let outline_inset = if params.view_options.show_outline_sidebar {
        params.view_options.outline_sidebar_width_dip.max(0.0)
    } else {
        0.0
    };
    let pane_rect = (0.0, 0.0, viewport_w, viewport_h);
    // §28 — display-row content height (scrollbar-consistent).
    let minimap_content_h =
        crate::scrollbar::content_height_for_scrollbar(params.frame_display, line_height);
    let layout = crate::minimap::compute_minimap_layout(
        pane_rect,
        scroll_y,
        line_height,
        rope.len_lines() as u64,
        minimap_content_h,
        outline_inset,
    );
    let colors = crate::minimap::MinimapColors {
        bg: params.colors.minimap_bg,
        fg: params.colors.minimap_fg,
        viewport_indicator: params.colors.minimap_viewport_indicator,
    };
    let _ = crate::minimap_paint::paint_minimap_scaled(
        &renderer.d2d_context,
        &renderer.dwrite_factory,
        params.format,
        rope,
        &layout,
        colors,
    );
    super::timing::elapsed_us(started)
}
