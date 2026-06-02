//! Brush factory shared by overlay paint modules.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{ID2D1RenderTarget, ID2D1SolidColorBrush};

use crate::{Error, Rgba};

/// On-demand `ID2D1SolidColorBrush` factory bound to one render target.
///
/// Shared by overlay sibling paint modules so they do not duplicate the
/// `CreateSolidColorBrush` boilerplate.
pub(crate) struct BrushCache {
    render_target: ID2D1RenderTarget,
}

impl BrushCache {
    pub(crate) fn new(render_target: &ID2D1RenderTarget) -> Result<Self, Error> {
        Ok(Self {
            render_target: render_target.clone(),
        })
    }

    pub(crate) fn solid(&mut self, color: Rgba) -> Result<ID2D1SolidColorBrush, Error> {
        let d2d_color: D2D1_COLOR_F = color.into();
        unsafe {
            self.render_target
                .CreateSolidColorBrush(&d2d_color, None)
                .map_err(Error::Graphics)
        }
    }
}
