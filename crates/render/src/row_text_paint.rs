//! Fixed-row text drawing guard.
//!
//! Text layouts may contain scaled markdown glyphs, but editor geometry
//! deliberately keeps one constant display-row stride. This painter clips
//! glyph output to that row so visual styling cannot bleed into the next
//! row or change scroll and caret coordinates.

use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

const ROW_CLIP_RIGHT_DIP: f32 = 1_000_000.0;

pub(crate) fn draw_text_layout_in_fixed_row(
    device_context: &ID2D1DeviceContext,
    layout: &IDWriteTextLayout,
    brush: &ID2D1SolidColorBrush,
    line_height_dip: f32,
) {
    let clip = compute_row_clip(line_height_dip);
    unsafe {
        device_context.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        device_context.DrawTextLayout(
            D2D_POINT_2F { x: 0.0, y: 0.0 },
            layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
        device_context.PopAxisAlignedClip();
    }
}

fn compute_row_clip(line_height_dip: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: 0.0,
        top: 0.0,
        right: ROW_CLIP_RIGHT_DIP,
        bottom: line_height_dip.max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::compute_row_clip;

    #[test]
    fn row_clip_tracks_runtime_height_and_clamps_negative_values() {
        assert_eq!(compute_row_clip(22.5).bottom, 22.5);
        assert_eq!(compute_row_clip(-3.0).bottom, 0.0);
        assert!(compute_row_clip(22.5).right.is_finite());
    }
}
