//! Phase G4 — paint the search-active minimap strip on the right edge
//! of the focused pane while the find bar is open.
//!
//! Geometry comes from the UI layer's pure `MinimapLayout` builder
//! (`crates/ui/src/search_minimap.rs`), mapped into the
//! [`crate::SearchMinimapDraw`] payload at frame build time. The
//! painter here is intentionally trivial: a translucent background
//! rect, then one short colored rect per match tick, with the
//! currently-focused tick painted in `match_active` and a touch wider
//! so the user can see which match the find bar is on.
//!
//! No DirectWrite text. No text layouts. No per-line work. Cheap
//! D2D solid-color brush fills only — per the spec ("DirectWrite/D2D-
//! painted colored rects — cheap, no full minimap").

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1RenderTarget, ID2D1SolidColorBrush,
};

use crate::params::{Rgba, SearchMinimapDraw};
use crate::Error;

/// Strip width in DIPs. Mirrors
/// `continuity_ui::search_minimap::SEARCH_MINIMAP_WIDTH_DIP` (the
/// layer graph forbids `render → ui`, so the constant lives here too
/// and the UI side is kept in sync by hand).
pub const SEARCH_MINIMAP_WIDTH_DIP: f32 = 12.0;

/// Extra DIPs added to the focused-match tick's width so it pops
/// against the surrounding ticks even at a glance.
const ACTIVE_TICK_EXTRA_WIDTH_DIP: f32 = 4.0;

/// Paint the search-active minimap strip described by `draw`.
///
/// Caller positions the device context's transform so `(0, 0)` is the
/// pane body's top-left in client coords. The painter then draws at
/// `draw.x_dip`/`draw.y_dip` relative to that origin.
///
/// # Errors
///
/// Returns [`Error::Graphics`] if a solid-color brush fails to allocate.
pub(crate) fn paint_search_minimap(
    ctx: &ID2D1DeviceContext,
    draw: &SearchMinimapDraw,
) -> Result<(), Error> {
    let render_target: ID2D1RenderTarget = ctx.cast()?;
    let mk_brush = |rgba: Rgba| -> Result<ID2D1SolidColorBrush, Error> {
        Ok(unsafe { render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(rgba), None)? })
    };
    let bg_brush = mk_brush(draw.bg)?;
    let match_brush = mk_brush(draw.match_color)?;
    let active_brush = mk_brush(draw.match_active)?;

    let strip = D2D_RECT_F {
        left: draw.x_dip,
        top: draw.y_dip,
        right: draw.x_dip + draw.width_dip,
        bottom: draw.y_dip + draw.height_dip,
    };
    unsafe { ctx.FillRectangle(&strip, &bg_brush) };

    for tick in &draw.ticks {
        let extra = if tick.is_active {
            ACTIVE_TICK_EXTRA_WIDTH_DIP
        } else {
            0.0
        };
        let brush = if tick.is_active {
            &active_brush
        } else {
            &match_brush
        };
        // Center the (wider) active tick on the strip's vertical axis.
        let left = (draw.x_dip - extra * 0.5).max(0.0);
        let right = left + draw.width_dip + extra;
        let top = draw.y_dip + tick.y_dip;
        let bottom = top + tick.height_dip;
        let rect = D2D_RECT_F {
            left,
            top,
            right,
            bottom,
        };
        unsafe { ctx.FillRectangle(&rect, brush) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::SearchMinimapTickDraw;

    /// `paint_search_minimap` needs an `ID2D1DeviceContext` we cannot
    /// build in a unit test (no swap chain). The behavior is covered
    /// by the pixel-canary integration test once a `find_bar_open=true`
    /// frame is captured.
    #[test]
    fn search_minimap_draw_is_constructible() {
        let d = SearchMinimapDraw {
            x_dip: 788.0,
            y_dip: 0.0,
            width_dip: 12.0,
            height_dip: 600.0,
            ticks: vec![SearchMinimapTickDraw {
                y_dip: 100.0,
                height_dip: 2.0,
                is_active: true,
            }],
            bg: Rgba::TRANSPARENT,
            match_color: Rgba::BLACK,
            match_active: Rgba::BLACK,
            body_highlights: Vec::new(),
        };
        assert_eq!(d.ticks.len(), 1);
        assert!(d.ticks[0].is_active);
    }
}
