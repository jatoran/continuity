//! α.1 edit-action echo painter.
//!
//! Paints a flat alpha-tinted band across one or more source rows after a
//! structural edit, undo/redo, or smart-expand step. Mirrors the jump-glow
//! painter in shape but covers a `[first_line, last_line]` range so a
//! multi-line paste, duplicate-line, or paragraph-reflow all surface a
//! visible echo without a separate motion mechanism.

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1DeviceContext, ID2D1RenderTarget};

use crate::motion::EditPulseDraw;
use crate::Error;

/// Paint a multi-row tint over `[first_line, last_line]`.
///
/// # Errors
///
/// Returns [`Error::Graphics`] if brush creation fails.
pub(crate) fn paint_edit_pulse(
    d2d: &ID2D1DeviceContext,
    draw: EditPulseDraw,
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
    let first = draw.first_line.min(draw.last_line) as f32;
    let last = draw.first_line.max(draw.last_line) as f32;
    let top = body_origin.1 + first * line_height - scroll_y;
    let bottom = body_origin.1 + (last + 1.0) * line_height - scroll_y;
    let viewport_bottom = body_origin.1 + viewport_h.max(1.0);
    if bottom < body_origin.1 || top > viewport_bottom {
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
        top: top.max(body_origin.1),
        right: body_origin.0 + viewport_w.max(1.0),
        bottom: bottom.min(viewport_bottom),
    };
    unsafe {
        d2d.FillRectangle(&rect, &brush);
    }
    Ok(())
}
