//! WARP-backed renderer constructor and back-buffer capture for the §D
//! pixel canary. Production code uses [`Renderer::for_hwnd`] which
//! prefers hardware D3D11 (hardware drivers do not produce
//! byte-identical output across machines, so they cannot back a hash
//! comparison). The canary uses [`Renderer::for_hwnd_warp`] +
//! [`Renderer::capture_back_buffer`] to render through the CPU
//! rasterizer and read the swap chain's back buffer into a CPU-side
//! `Vec<u8>` for hashing.

use std::mem::ManuallyDrop;
use std::ptr::copy_nonoverlapping;

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Resource, ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::renderer::Renderer;
use crate::text_helpers::create_d3d11_device_warp_only;
use crate::Error;

/// CPU-side BGRA8 snapshot of a renderer's back buffer.
///
/// Layout: `bgra.len() == width * height * 4`, top-down rows, no
/// padding. Channel order is BGRA (matches the swap chain format
/// `DXGI_FORMAT_B8G8R8A8_UNORM`).
pub struct CapturedBitmap {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Tight-packed BGRA bytes, top-down.
    pub bgra: Vec<u8>,
}

impl Renderer {
    /// Build a renderer backed by the WARP software rasterizer. Used by
    /// the §D pixel canary so frame hashes don't depend on the host
    /// GPU. Production callers should use [`Renderer::for_hwnd`]
    /// which prefers hardware.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] for any underlying Win32 call failure.
    pub fn for_hwnd_warp(hwnd: HWND, width: u32, height: u32) -> Result<Self, Error> {
        let (device, context) = create_d3d11_device_warp_only()?;
        let dxgi_device: IDXGIDevice = device.cast()?;
        let dxgi_factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1()? };

        let swap_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swap_chain: IDXGISwapChain1 =
            unsafe { dxgi_factory.CreateSwapChainForHwnd(&device, hwnd, &swap_desc, None, None)? };

        let d2d_factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let d2d_device = unsafe { d2d_factory.CreateDevice(&dxgi_device)? };
        let d2d_context =
            unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };

        let surface: IDXGISurface = unsafe { swap_chain.GetBuffer(0)? };
        let dpi = continuity_win::dpi_for_window(hwnd) as f32;
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
        let bitmap =
            unsafe { d2d_context.CreateBitmapFromDxgiSurface(&surface, Some(&bitmap_props))? };
        unsafe {
            d2d_context.SetTarget(&bitmap);
            // Bind the drawing transform to the capture DPI (see
            // `construction.rs`). The canary captures at 96 DPI, so this is a
            // no-op there, but it keeps both constructor paths identical and
            // correct if a high-DPI capture is ever taken.
            d2d_context.SetDpi(dpi, dpi);
            // Force grayscale text antialiasing for byte determinism.
            // ClearType subpixel positioning depends on per-channel
            // glyph offsets that vary by font version + LCD orientation;
            // grayscale produces stable BGRA output that can be hashed
            // across machines.
            d2d_context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }

        let dwrite_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)? };

        Ok(Self {
            device,
            context,
            swap_chain,
            _d2d_factory: d2d_factory,
            d2d_context,
            target_width_px: width.max(1),
            target_height_px: height.max(1),
            target_dpi: dpi.max(1.0),
            _target_bitmap: Some(bitmap),
            dwrite_factory,
            image_cache: std::cell::RefCell::new(crate::image_cache::ImageCache::new(0)),
            last_image_hits: std::cell::RefCell::new(Vec::new()),
            last_inline_code_hits: std::cell::RefCell::new(Vec::new()),
            last_body_paint_us: std::cell::Cell::new(0),
            last_post_body_paint_us: std::cell::Cell::new(0),
            last_post_body_stages: std::cell::Cell::new(crate::RendererPostBodyStages::default()),
            chrome_command_list: std::cell::RefCell::new(
                crate::chrome_command_list::ChromeCommandList::default(),
            ),
            last_chrome_path_stats: std::cell::Cell::new(crate::ChromePathStats::default()),
            table_chrome_cache: std::cell::RefCell::new(
                crate::table_chrome_cache::TableChromeCache::default(),
            ),
            last_table_chrome_stats: std::cell::Cell::new(crate::TableChromePathStats::default()),
            last_scroll_placeholder_rows: std::cell::Cell::new(0),
            last_scroll_strip_rows: std::cell::Cell::new(0),
            last_chrome_overlay_breakdown: std::cell::Cell::new(
                crate::RendererChromeOverlayBreakdown::default(),
            ),
            last_soft_wrap_overflow: std::cell::Cell::new(crate::SoftWrapOverflowSample::default()),
        })
    }

    /// Copy the current swap chain back buffer into a CPU-readable
    /// staging texture and return its BGRA bytes. Must be called
    /// **before** the next `Present` (flip-discard semantics
    /// invalidate the back buffer after Present).
    ///
    /// The §D canary calls this after `draw_buffer_no_present` so the
    /// pixels reflect the just-submitted draw without the additional
    /// frame motion that Present would introduce.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] for any underlying Win32 call failure.
    pub fn capture_back_buffer(&self) -> Result<CapturedBitmap, Error> {
        let back_buffer: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0)? };

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { back_buffer.GetDesc(&mut desc) };

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };

        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        }
        let staging = staging.ok_or_else(|| Error::Graphics(windows::core::Error::from_win32()))?;

        let staging_resource: ID3D11Resource = staging.cast()?;
        let source_resource: ID3D11Resource = back_buffer.cast()?;
        unsafe {
            self.context
                .CopyResource(&staging_resource, &source_resource);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging_resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }

        let width = desc.Width;
        let height = desc.Height;
        let bytes_per_row = (width as usize) * 4;
        let row_pitch = mapped.RowPitch as usize;

        let mut bgra = vec![0u8; bytes_per_row * height as usize];
        unsafe {
            for row in 0..height as usize {
                let src = (mapped.pData as *const u8).add(row * row_pitch);
                let dst = bgra.as_mut_ptr().add(row * bytes_per_row);
                copy_nonoverlapping(src, dst, bytes_per_row);
            }
            self.context.Unmap(&staging_resource, 0);
        }

        Ok(CapturedBitmap {
            width,
            height,
            bgra,
        })
    }
}
