//! Focus-mode dim pass — extracted from [`crate::Renderer::draw_buffer_no_present`]
//! so the renderer file stays under the conventions cap.
//!
//! Composes [`crate::focus_dim`] primitives: computes the focused span
//! from the caret, derives the dim row rectangles, and paints them in
//! body-content space. No-op when focus mode is off or the dim span has
//! no rows in the visible range.
//!
//! Thread ownership: UI thread (sole owner of the D2D context).

use ropey::Rope;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush,
};

use crate::chrome::ContentMargins;
use crate::params::{DrawParams, Rgba};
use crate::Error;

/// Paint the focus-mode dim overlay over rows outside the focused span.
/// Returns `Ok(())` and is a no-op when focus mode is disabled or the
/// computed span produces no visible dim rows.
// One paint phase that needs the full render-frame context — bundling
// these into a struct would just be a one-shot record.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_focus_dim_pass(
    device_context: &ID2D1DeviceContext,
    render_target: &ID2D1RenderTarget,
    rope: &Rope,
    selections: &[continuity_text::Selection],
    params: &DrawParams<'_>,
    margins: ContentMargins,
    body_translate: Matrix3x2,
    line_height: f32,
    scroll_y: f32,
    editor_w: f32,
    first_visible: usize,
    last_visible: usize,
) -> Result<(), Error> {
    if params.view_options.focus_mode.is_empty()
        || params.view_options.focus_mode == "off"
        || params.view_options.focus_dim_alpha <= f32::EPSILON
    {
        return Ok(());
    }
    let caret_byte = selections
        .first()
        .map(|s| {
            let line = s.head.line as usize;
            let line_start = if line < rope.len_lines() {
                rope.line_to_byte(line)
            } else {
                rope.len_bytes()
            };
            line_start + s.head.byte_in_line as usize
        })
        .unwrap_or(0);
    // The `to_string` is the only allocation; rope.to_string amortizes
    // to a single contiguous walk over the rope's leaves, so it's cheap
    // for the buffer sizes the editor is provisioned for (focus-mode is
    // opt-in and the user already paid the rope walk in the body glyph
    // pass).
    let source = rope.to_string();
    let Some(focus_span) =
        crate::focus_dim::compute_focus_span(&source, caret_byte, params.view_options.focus_mode)
    else {
        return Ok(());
    };
    let total_display = params.frame_display.display_line_count();
    let last = (last_visible as u32).min(total_display);
    let dim_rows = crate::focus_dim::compute_dim_rows(
        rope,
        params.frame_display.map(),
        focus_span,
        line_height,
        scroll_y,
        first_visible as u32,
        last,
    );
    if dim_rows.is_empty() {
        return Ok(());
    }
    let dim_brush_color: D2D1_COLOR_F = Rgba {
        r: params.view_options.focus_dim_color.r,
        g: params.view_options.focus_dim_color.g,
        b: params.view_options.focus_dim_color.b,
        a: params.view_options.focus_dim_color.a * params.view_options.focus_dim_alpha,
    }
    .into();
    let dim_brush: ID2D1SolidColorBrush =
        unsafe { render_target.CreateSolidColorBrush(&dim_brush_color, None)? };
    // Paint in body-content space (x = 0 = body left).
    let body_content_translate = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: body_translate.M31 + margins.left,
        M32: body_translate.M32,
    };
    unsafe {
        device_context.SetTransform(&body_content_translate);
    }
    let _ = crate::focus_dim::paint_focus_dim(device_context, &dim_rows, 0.0, editor_w, &dim_brush);
    unsafe {
        device_context.SetTransform(&body_translate);
    }
    Ok(())
}
