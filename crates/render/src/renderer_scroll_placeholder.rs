//! Scroll-tick placeholder strip paint pass — extracted from
//! [`crate::Renderer::draw_buffer_no_present`] so that file stays under
//! the 600-line conventions cap.
//!
//! See [`crate::scroll_placeholder`] for the pure strip-computation
//! helpers; this module owns the D2D brush construction and the call
//! into the strip painter. The body-level transform must already be
//! installed by the caller — strip rects are interpreted in
//! body-relative DIPs (`y = row * line_height - scroll_y`).
//!
//! Thread ownership: UI thread (D2D handles).

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1RenderTarget};

use crate::params::{DrawParams, Rgba};
use crate::scroll_placeholder::{
    compute_unrealized_strips, paint_scroll_placeholder_strips, placeholder_row_count,
};
use crate::Error;

/// Compute and paint the placeholder strips covering live-viewport
/// display rows that the painted [`crate::FrameDisplay`] has not
/// realized. Returns the total row count painted as placeholder (zero
/// when the frame fully covers the viewport).
///
/// Callers stamp the returned count onto `Renderer::last_scroll_placeholder_rows`
/// so the per-paint `event:scroll_path` trace can emit
/// `rows_placeholder=N`.
///
/// # Errors
///
/// Returns [`Error::Graphics`] when the brush creation underlying the
/// fill call fails.
pub(crate) fn paint_scroll_placeholder_pass(
    device_context: &ID2D1DeviceContext,
    render_target: &ID2D1RenderTarget,
    params: &DrawParams<'_>,
    line_height: f32,
    scroll_y: f32,
    viewport_h: f32,
    body_right_dip: f32,
) -> Result<u32, Error> {
    let total_display = params.frame_display.display_line_count() as i64;
    let visible_start = ((scroll_y / line_height).floor() as i64).max(0) as u32;
    let visible_end = ((((scroll_y + viewport_h) / line_height).ceil() as i64) + 1)
        .clamp(0, total_display) as u32;
    let visible = visible_start..visible_end;
    let realized = params.frame_display.realized_row_range();
    let strips = compute_unrealized_strips(realized, visible, line_height, scroll_y);
    if strips.is_empty() {
        return Ok(0);
    }
    let color = scroll_placeholder_color(params.colors.loading_overlay_bg);
    let color_d2d: D2D1_COLOR_F = color.into();
    let brush = unsafe { render_target.CreateSolidColorBrush(&color_d2d, None)? };
    unsafe {
        paint_scroll_placeholder_strips(device_context, &strips, 0.0, body_right_dip, &brush);
    }
    Ok(placeholder_row_count(&strips))
}

/// Color for the scroll-tick placeholder strip. Prefers the
/// theme-supplied `editor.loading_overlay.background` when it has any
/// alpha; falls back to a soft semi-transparent grey so a theme with
/// zero-alpha loading overlay still surfaces the strip.
pub(crate) fn scroll_placeholder_color(theme_overlay_bg: Rgba) -> Rgba {
    if theme_overlay_bg.a > f32::EPSILON {
        // Apply a small alpha attenuation so a heavy loading-overlay
        // theme color does not paint a hard band over the body.
        Rgba {
            a: theme_overlay_bg.a * 0.6,
            ..theme_overlay_bg
        }
    } else {
        Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 0.06,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonzero_theme_overlay_is_attenuated() {
        let color = scroll_placeholder_color(Rgba {
            r: 0.9,
            g: 0.9,
            b: 0.9,
            a: 0.4,
        });
        // Same RGB, alpha scaled by 0.6.
        assert!((color.r - 0.9).abs() < 1e-6);
        assert!((color.g - 0.9).abs() < 1e-6);
        assert!((color.b - 0.9).abs() < 1e-6);
        assert!((color.a - 0.24).abs() < 1e-6);
    }

    #[test]
    fn zero_alpha_theme_overlay_falls_back_to_neutral_grey() {
        let color = scroll_placeholder_color(Rgba::default());
        assert!((color.r - 0.5).abs() < 1e-6);
        assert!((color.g - 0.5).abs() < 1e-6);
        assert!((color.b - 0.5).abs() < 1e-6);
        assert!((color.a - 0.06).abs() < 1e-6);
    }
}
