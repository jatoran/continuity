//! Small [`crate::Renderer`] helpers kept out of the draw orchestrator.

use ropey::Rope;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;

use crate::params::Rgba;
use crate::{Error, Renderer};

impl Renderer {
    /// Clear the back buffer to `color` and present.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] if BeginDraw / EndDraw / Present fails.
    pub fn present_clear(&self, color: Rgba) -> Result<(), Error> {
        let d2d_color: D2D1_COLOR_F = color.into();
        unsafe {
            self.d2d_context.BeginDraw();
            self.d2d_context.Clear(Some(&d2d_color));
            self.d2d_context.EndDraw(None, None)?;
            self.swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    /// Present the current back buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] if DXGI present fails.
    pub fn present(&self) -> Result<(), Error> {
        unsafe {
            self.swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    /// Raw source-line content height in DIPs for `rope`.
    ///
    /// Scrollbar paint uses the frame-display row index instead so
    /// soft-wrap and folds match hit testing.
    #[must_use]
    pub fn content_height(rope: &Rope, line_height: f32) -> f32 {
        rope.len_lines().max(1) as f32 * line_height
    }

    /// Clear per-frame state that only the soft-wrap body pass populates.
    pub(crate) fn clear_unwrapped_frame_state(&self) {
        self.last_inline_code_hits.borrow_mut().clear();
        self.last_soft_wrap_overflow
            .set(crate::SoftWrapOverflowSample::default());
    }
}
