//! Transient "building view" overlay drawn over a stale frame while paint
//! waits for a slow projection-worker build to publish.
//!
//! The overlay is a banner-style, semi-transparent panel centered near
//! the top of the focused pane body. Painted from
//! [`crate::renderer_post_body`] after the status bar and before any
//! modal overlay so it sits above body chrome but below palette / find
//! surfaces. The rope is canonical — this surface adds no buffer
//! content; it is a chrome cue that the next paint will be a worker hit.
//!
//! Thread ownership: the caller is the UI thread (the only owner of
//! the `ID2D1DeviceContext`).

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_MEASURING_MODE_NATURAL,
};

use crate::motion::SurfaceMotion;
use crate::{Error, Rgba};

/// Default overlay width in DIPs. Sized to comfortably fit the label
/// without dominating the pane body.
pub const LOADING_OVERLAY_WIDTH_DIP: f32 = 220.0;

/// Default overlay height in DIPs.
pub const LOADING_OVERLAY_HEIGHT_DIP: f32 = 44.0;

/// Top offset from the focused pane body's top edge in DIPs. Sits
/// near the top so it does not occlude the caret line, which is
/// typically near the middle of the viewport.
pub const LOADING_OVERLAY_TOP_OFFSET_DIP: f32 = 28.0;

/// Corner radius in DIPs.
pub const LOADING_OVERLAY_CORNER_RADIUS_DIP: f32 = 6.0;

/// Plain-data payload for one frame's loading overlay paint.
///
/// Fields are populated by the UI layer once per paint when the
/// loading-overlay state is armed; built fresh per frame so the
/// renderer holds no mutable surface state.
#[derive(Clone, Debug)]
pub struct LoadingOverlayDraw {
    /// Pane-body-relative top-left of the panel in DIPs. Renderer
    /// adds `body_origin` before issuing draws so the overlay lands
    /// inside the focused pane regardless of pane layout.
    pub x_dip: f32,
    /// Pane-body-relative top edge.
    pub y_dip: f32,
    /// Panel width in DIPs.
    pub width_dip: f32,
    /// Panel height in DIPs.
    pub height_dip: f32,
    /// Corner radius in DIPs.
    pub corner_radius_dip: f32,
    /// Translucent panel fill.
    pub bg: Rgba,
    /// Foreground color of the label text.
    pub fg: Rgba,
    /// Optional 1-DIP border. `a = 0.0` skips the stroke.
    pub border: Rgba,
    /// Label text. Neutral copy — no moralizing per
    /// `.docs/design/principles.md` ("trust the writer").
    pub label: String,
}

impl LoadingOverlayDraw {
    /// Build a centered-near-top overlay payload using the renderer's
    /// default geometry. `pane_body_width_dip` centers the panel
    /// horizontally inside the focused pane body.
    #[must_use]
    pub fn centered(
        pane_body_width_dip: f32,
        bg: Rgba,
        fg: Rgba,
        border: Rgba,
        label: impl Into<String>,
    ) -> Self {
        let width_dip = LOADING_OVERLAY_WIDTH_DIP.min(pane_body_width_dip.max(0.0));
        let x_dip = ((pane_body_width_dip - width_dip) * 0.5).max(0.0);
        Self {
            x_dip,
            y_dip: LOADING_OVERLAY_TOP_OFFSET_DIP,
            width_dip,
            height_dip: LOADING_OVERLAY_HEIGHT_DIP,
            corner_radius_dip: LOADING_OVERLAY_CORNER_RADIUS_DIP,
            bg,
            fg,
            border,
            label: label.into(),
        }
    }
}

/// Paint `draw` onto `ctx` at `body_origin`. Must be called inside an
/// active `BeginDraw`/`EndDraw` bracket.
///
/// `motion` projects opacity and a small vertical translation when the
/// overlay is animating in. When `motion.opacity <= 0`, the call is a
/// no-op so a reduced-motion + just-armed combination cannot flash.
///
/// # Errors
///
/// Returns [`Error::Graphics`] on any underlying Win32 failure.
pub fn paint_loading_overlay(
    ctx: &ID2D1DeviceContext,
    _dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    draw: &LoadingOverlayDraw,
    body_origin: (f32, f32),
    motion: SurfaceMotion,
) -> Result<(), Error> {
    if motion.opacity <= f32::EPSILON {
        return Ok(());
    }
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let opacity = motion.opacity.clamp(0.0, 1.0);
    let bg = fade(draw.bg, opacity);
    let fg = fade(draw.fg, opacity);
    let border = fade(draw.border, opacity);
    let left = body_origin.0 + draw.x_dip;
    let top = body_origin.1 + draw.y_dip + motion.translate_y_dip;
    let right = left + draw.width_dip.max(0.0);
    let bottom = top + draw.height_dip.max(0.0);
    let panel_rect = D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    };
    let rounded = D2D1_ROUNDED_RECT {
        rect: panel_rect,
        radiusX: draw.corner_radius_dip,
        radiusY: draw.corner_radius_dip,
    };
    let bg_color: D2D1_COLOR_F = bg.into();
    let bg_brush = unsafe { render_target.CreateSolidColorBrush(&bg_color, None)? };
    unsafe {
        ctx.FillRoundedRectangle(&rounded, &bg_brush);
    }
    if border.a > 0.0 {
        let border_color: D2D1_COLOR_F = border.into();
        let border_brush = unsafe { render_target.CreateSolidColorBrush(&border_color, None)? };
        unsafe {
            ctx.DrawRoundedRectangle(&rounded, &border_brush, 1.0, None);
        }
    }
    if !draw.label.is_empty() {
        let fg_color: D2D1_COLOR_F = fg.into();
        let fg_brush = unsafe { render_target.CreateSolidColorBrush(&fg_color, None)? };
        let wide: Vec<u16> = draw.label.encode_utf16().collect();
        let text_rect = D2D_RECT_F {
            left: left + 12.0,
            top,
            right: right - 12.0,
            bottom,
        };
        unsafe {
            ctx.DrawText(
                &wide,
                format,
                &text_rect,
                &fg_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    Ok(())
}

fn fade(color: Rgba, opacity: f32) -> Rgba {
    Rgba {
        a: color.a * opacity,
        ..color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_pins_within_pane_width() {
        let draw = LoadingOverlayDraw::centered(
            600.0,
            Rgba::default(),
            Rgba::default(),
            Rgba::default(),
            "Loading view",
        );
        assert_eq!(draw.width_dip, LOADING_OVERLAY_WIDTH_DIP);
        assert!((draw.x_dip - (600.0 - LOADING_OVERLAY_WIDTH_DIP) / 2.0).abs() < 1e-3);
        assert_eq!(draw.label, "Loading view");
    }

    #[test]
    fn centered_clamps_to_narrow_pane() {
        let draw = LoadingOverlayDraw::centered(
            100.0,
            Rgba::default(),
            Rgba::default(),
            Rgba::default(),
            "Loading view",
        );
        assert!(draw.width_dip <= 100.0);
        assert_eq!(draw.x_dip, 0.0);
    }

    #[test]
    fn fade_scales_alpha_only() {
        let color = Rgba {
            r: 0.5,
            g: 0.6,
            b: 0.7,
            a: 1.0,
        };
        let faded = fade(color, 0.5);
        assert!((faded.a - 0.5).abs() < 1e-6);
        assert_eq!(faded.r, 0.5);
        assert_eq!(faded.g, 0.6);
        assert_eq!(faded.b, 0.7);
    }
}
