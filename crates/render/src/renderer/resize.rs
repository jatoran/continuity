//! [`Renderer::resize`] — reuse the existing D3D11 / D2D / DirectWrite
//! stack across `WM_SIZE` ticks. Only the resize-dependent swap-chain
//! target resources are recreated.
//!
//! Live drag-resize used to set `Window::renderer = None`, which forced
//! `ensure_renderer` to rebuild the entire renderer (D3D11 device + DXGI
//! factory + swap chain + D2D factory/device/context + target bitmap +
//! DirectWrite factory + image cache) on the next paint. That is far
//! too expensive for every `WM_SIZE` during a drag. This module rebinds
//! only what the new swap-chain dimensions or per-window DPI invalidate.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Image, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN};
use windows::Win32::Graphics::Dxgi::{IDXGISurface, DXGI_SWAP_CHAIN_FLAG};

use crate::renderer::Renderer;
use crate::Error;

impl Renderer {
    /// Current swap-chain back-buffer size in physical pixels.
    #[must_use]
    pub fn back_buffer_size(&self) -> (u32, u32) {
        (self.target_width_px, self.target_height_px)
    }

    /// Resize the swap chain to `(width, height)` pixels, reusing the
    /// underlying D3D11 device, DXGI factory, D2D device/context,
    /// DirectWrite factory, and image cache. The new target bitmap reads
    /// the live per-window DPI from `hwnd`.
    ///
    /// Sequence:
    /// 1. Clear the D2D target so the old bitmap holds the last
    ///    reference to the back-buffer surface.
    /// 2. Drop the old target bitmap (`ResizeBuffers` fails while any
    ///    outstanding back-buffer reference is alive).
    /// 3. `IDXGISwapChain::ResizeBuffers` — preserves buffer count,
    ///    format, and flags.
    /// 4. `GetBuffer(0)` → fresh `IDXGISurface`.
    /// 5. `CreateBitmapFromDxgiSurface` → fresh `ID2D1Bitmap1`.
    /// 6. `SetTarget` to the new bitmap and store it on `self`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] if any underlying Win32 call fails.
    /// On failure the renderer is left without a bound target; callers
    /// (currently `Window::refresh_client_size`) drop the renderer in
    /// that case so the next paint goes through the cold construction
    /// path.
    pub fn resize_for_hwnd(&mut self, hwnd: HWND, width: u32, height: u32) -> Result<(), Error> {
        let dpi = continuity_win::dpi_for_window(hwnd) as f32;
        self.resize_with_dpi(width, height, dpi)
    }

    /// Resize using the legacy 96-DPI target. Kept for tests that bind a
    /// renderer to a synthetic surface without a real DPI transition.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Error> {
        self.resize_with_dpi(width, height, 96.0)
    }

    fn resize_with_dpi(&mut self, width: u32, height: u32, dpi: f32) -> Result<(), Error> {
        let width = width.max(1);
        let height = height.max(1);
        let dpi = dpi.max(1.0);
        if self.target_width_px == width
            && self.target_height_px == height
            && (self.target_dpi - dpi).abs() < f32::EPSILON
        {
            return Ok(());
        }
        self.chrome_command_list.borrow_mut().invalidate();
        self.table_chrome_cache.borrow_mut().invalidate();

        unsafe {
            self.d2d_context.SetTarget(None::<&ID2D1Image>);
        }
        // Drop the previous back-buffer bitmap *before* ResizeBuffers.
        // The D2D target reference released above plus this drop are
        // the only outstanding references to buffer 0; without both,
        // ResizeBuffers returns DXGI_ERROR_INVALID_CALL.
        drop(self._target_bitmap.take());

        unsafe {
            self.swap_chain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }

        let surface: IDXGISurface = unsafe { self.swap_chain.GetBuffer(0)? };
        let bitmap_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        let bitmap = unsafe {
            self.d2d_context
                .CreateBitmapFromDxgiSurface(&surface, Some(&bitmap_props))?
        };
        unsafe {
            self.d2d_context.SetTarget(&bitmap);
        }
        self.target_width_px = width;
        self.target_height_px = height;
        self.target_dpi = dpi;
        self._target_bitmap = Some(bitmap);
        Ok(())
    }
}
