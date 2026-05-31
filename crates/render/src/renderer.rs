//! `Renderer`: D3D11 + DXGI + D2D bitmap target bound to one HWND. Routes
//! visible lines through `continuity_layout::LayoutCache` and presents
//! one frame's worth of D2D commands per call.

use continuity_layout::LayoutCache;
use continuity_text::Selection;
use ropey::Rope;
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain1, DXGI_PRESENT};

use crate::params::DrawParams;
use crate::Error;

/// A renderer bound to an HWND. Holds D3D11, DXGI, D2D, and DirectWrite
/// resources.
/// Fields are `pub(crate)` so the sibling [`crate::renderer_capture`]
/// module can reach the D3D context + swap chain to copy the back buffer
/// into a CPU-readable staging texture (used by the §D pixel canary),
/// and so the [`crate::renderer_draw_main`] sibling owns the actual
/// frame-submission body.
pub struct Renderer {
    pub(crate) device: ID3D11Device,
    pub(crate) context: ID3D11DeviceContext,
    pub(crate) swap_chain: IDXGISwapChain1,
    pub(crate) _d2d_factory: ID2D1Factory1,
    pub(crate) d2d_context: ID2D1DeviceContext,
    /// Current swap-chain back-buffer pixel width.
    pub(crate) target_width_px: u32,
    /// Current swap-chain back-buffer pixel height.
    pub(crate) target_height_px: u32,
    /// DPI used when the current D2D target bitmap was created.
    pub(crate) target_dpi: f32,
    /// Swap-chain back-buffer bound as the D2D target. `Option` so
    /// [`Self::resize`] can drop it before `ResizeBuffers` runs
    /// (`IDXGISwapChain::ResizeBuffers` fails while any outstanding
    /// reference to the back-buffer surface is alive).
    pub(crate) _target_bitmap: Option<ID2D1Bitmap1>,
    pub(crate) dwrite_factory: IDWriteFactory,
    /// Phase F5 Pass 2 — inline-image bitmap cache. Interior
    /// mutability keeps the renderer's draw API on `&self`. Capacity
    /// is updated by the window via
    /// [`Self::set_image_cache_capacity`] when settings hot-reload.
    pub(crate) image_cache: std::cell::RefCell<crate::ImageCache>,
    /// F5 redesign — per-frame hit-test rects for the collapsed
    /// inline-image affordances. Filled by `paint_inline_images`
    /// each frame; consumed by the UI mouse handler. Always
    /// pane-body-relative coordinates.
    pub(crate) last_image_hits: std::cell::RefCell<Vec<crate::InlineImageHit>>,
    /// Per-frame hit rects for every painted inline-code span on the
    /// focused pane. Filled by `inline_code_paint` and consumed by
    /// the UI mouse handler that drives the inline copy-button
    /// hover affordance. Always client-DIP coordinates (the painter
    /// translates through the active body origin before pushing).
    pub(crate) last_inline_code_hits: std::cell::RefCell<Vec<crate::InlineCodeHit>>,
    /// Microseconds the most recent `draw_buffer_no_present` spent
    /// inside the body paint pass (`wrap_paint::paint_display_lines`
    /// or `renderer_line_text_pass::paint_line_text_pass`). UI reads
    /// after every draw and writes into [`crate::RenderStats`].
    pub(crate) last_body_paint_us: std::cell::Cell<u64>,
    /// Microseconds the most recent `draw_buffer_no_present` spent
    /// inside the post-body paint pass (status bar, chrome, scrollbar,
    /// line numbers, minimap, search strip, inline-image overlays).
    pub(crate) last_post_body_paint_us: std::cell::Cell<u64>,
    /// Post-body sub-stage timings from the most recent draw.
    pub(crate) last_post_body_stages: std::cell::Cell<crate::RendererPostBodyStages>,
    /// Device-resident retained static chrome command list. UI thread
    /// owns the renderer and therefore owns this mutable cache.
    pub(crate) chrome_command_list:
        std::cell::RefCell<crate::chrome_command_list::ChromeCommandList>,
    /// Static-chrome record/replay timing from the most recent draw.
    pub(crate) last_chrome_path_stats: std::cell::Cell<crate::ChromePathStats>,
    /// Device-resident per-table chrome command-list cache (P14.1).
    /// UI thread owns the renderer and therefore owns this mutable
    /// cache.
    pub(crate) table_chrome_cache: std::cell::RefCell<crate::table_chrome_cache::TableChromeCache>,
    /// Per-table chrome record/replay timing from the most recent draw.
    pub(crate) last_table_chrome_stats: std::cell::Cell<crate::TableChromePathStats>,
    /// Display-row count painted as a soft "loading" placeholder during
    /// the most recent draw because the realized row range of the
    /// painted [`crate::FrameDisplay`] did not cover the live viewport.
    /// Non-zero only on scroll-tick paints that reused the previous
    /// frame; UI reads via [`Self::last_scroll_placeholder_rows`] for
    /// the `event:scroll_path` trace payload.
    pub(crate) last_scroll_placeholder_rows: std::cell::Cell<u32>,
    /// Display-row count the dispatch synchronously realized on the
    /// most recent scroll-tick paint via the section-10 strip-realize
    /// path. UI stamps this via [`Self::set_last_scroll_strip_rows`]
    /// after [`Window::resolve_paint_frame_display`] returns and the
    /// dispatch trace reads it for `rows_realized_synchronously` in
    /// `event:scroll_path`.
    pub(crate) last_scroll_strip_rows: std::cell::Cell<u32>,
    /// Chrome-overlay sub-stage breakdown from the most recent draw.
    /// UI thread reads via [`Self::last_chrome_overlay_breakdown`] and
    /// stamps into [`crate::RenderStats`] for the chrome-overlay split
    /// fields on `event:renderer_draw_stages`.
    pub(crate) last_chrome_overlay_breakdown:
        std::cell::Cell<crate::RendererChromeOverlayBreakdown>,
}
mod construction;
mod resize;
impl Renderer {
    // Image-cache helpers live in `renderer_image_cache.rs`; per-draw
    // timing accessors live in `renderer_draw_stats.rs`; the body of
    // `draw_buffer_no_present` lives in `renderer_draw_main.rs`.

    /// Submit one frame's draw commands without calling `Present`. Used
    /// by the §B2 perf gate to measure pure draw-call submission cost
    /// (BeginDraw → command list → EndDraw) without the DXGI Present
    /// queue backpressure that flip-discard swap chains can introduce.
    /// Production callers should use [`Renderer::draw_buffer`] which
    /// adds the immediate Present at the end.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] if any underlying call fails.
    pub fn draw_buffer_no_present(
        &self,
        rope: &Rope,
        selections: &[Selection],
        cache: &mut LayoutCache,
        params: &DrawParams<'_>,
    ) -> Result<(), Error> {
        crate::renderer_draw_main::render_frame(self, rope, selections, cache, params)
    }

    /// Submit one frame's draw commands and immediately Present. The
    /// production frame loop calls this once per `WM_PAINT`. See
    /// [`Renderer::draw_buffer_no_present`] for the submission-only
    /// variant used by perf gates.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Graphics`] if any underlying call fails.
    pub fn draw_buffer(
        &self,
        rope: &Rope,
        selections: &[Selection],
        cache: &mut LayoutCache,
        params: &DrawParams<'_>,
    ) -> Result<(), Error> {
        self.draw_buffer_no_present(rope, selections, cache, params)?;
        unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)).ok()? };
        Ok(())
    }
}
