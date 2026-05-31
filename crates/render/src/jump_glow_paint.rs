//! Destination-row acknowledgement glow painter.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1RenderTarget};

use crate::motion::JumpGlowDraw;
use crate::Error;

/// Paint a single line-height tint over the destination row.
///
/// # Errors
///
/// Returns [`Error::Graphics`] if brush creation fails.
pub(crate) fn paint_jump_glow(
    d2d: &ID2D1DeviceContext,
    draw: JumpGlowDraw,
    body_origin: (f32, f32),
    viewport_w: f32,
    viewport_h: f32,
    line_height: f32,
    scroll_y: f32,
) -> Result<(), Error> {
    let alpha = draw.alpha.clamp(0.0, 1.0);
    if alpha <= f32::EPSILON {
        return Ok(());
    }
    let y = body_origin.1 + draw.display_line as f32 * line_height - scroll_y;
    if y + line_height < body_origin.1 || y > body_origin.1 + viewport_h.max(1.0) {
        // Avoid building brushes for clearly nonsensical offscreen values.
        return Ok(());
    }
    let color = crate::Rgba {
        a: draw.color.a * alpha,
        ..draw.color
    };
    let rt: ID2D1RenderTarget = d2d.cast()?;
    let brush = unsafe { rt.CreateSolidColorBrush(&D2D1_COLOR_F::from(color), None)? };
    let rect = D2D_RECT_F {
        left: body_origin.0,
        top: y,
        right: body_origin.0 + viewport_w.max(1.0),
        bottom: y + line_height,
    };
    unsafe {
        d2d.FillRectangle(&rect, &brush);
    }
    Ok(())
}
