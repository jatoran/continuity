//! [`Renderer::for_hwnd`] — D3D11 / DXGI / D2D / DirectWrite initialization
//! for a single HWND. The rest of the renderer's draw surface lives in the
//! parent `renderer.rs`; this sibling only owns the one-time construction
//! path so the hot-path file stays under the 600-line cap.

use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
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
use crate::text_helpers::create_d3d11_device;
use crate::Error;

impl Renderer {
    /// Build a renderer for the given HWND at the given pixel size,
    /// binding the D2D target at the HWND's current per-monitor DPI.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] for any underlying Win32 call failure.
    pub fn for_hwnd(hwnd: HWND, width: u32, height: u32) -> Result<Self, Error> {
        let (device, context) = create_d3d11_device()?;
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
            // The target *bitmap* carries `dpi`, but that only fixes the DIP
            // size the bitmap reports — it does not change how the device
            // context maps DIP draw coordinates to physical pixels. Without
            // an explicit `SetDpi`, the context paints at the default 96 DPI
            // (1 DIP = 1 px), so the DIP-native layout fills only
            // `client_px / scale` of the back buffer at high DPI (the dark
            // right/bottom band at 125%+). Bind the drawing transform to the
            // window DPI so DIPs scale to physical pixels.
            d2d_context.SetDpi(dpi, dpi);
        };

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
}
