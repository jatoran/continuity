//! Scrollbar paint helper for list overlays.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;

use crate::overlay::{BrushCache, OverlayScrollbar, Rect};
use crate::Error;

pub(crate) fn paint_overlay_scrollbar(
    ctx: &ID2D1DeviceContext,
    brushes: &mut BrushCache,
    scrollbar: &OverlayScrollbar,
) -> Result<(), Error> {
    let track_brush = brushes.solid(scrollbar.track_color)?;
    let thumb_brush = brushes.solid(scrollbar.thumb_color)?;
    unsafe {
        ctx.FillRectangle(&to_d2d(scrollbar.track), &track_brush);
        ctx.FillRectangle(&to_d2d(scrollbar.thumb), &thumb_brush);
    }
    Ok(())
}

fn to_d2d(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.w,
        bottom: rect.y + rect.h,
    }
}
