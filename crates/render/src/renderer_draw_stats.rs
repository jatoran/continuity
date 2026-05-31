//! Per-draw timing accessors on [`crate::Renderer`].
//!
//! Pulled out of `renderer.rs` to keep that file under the 600-line
//! conventions cap. `Renderer::draw_buffer_no_present` populates the
//! `last_body_paint_us` / `last_post_body_paint_us` cells during the
//! paint; UI's dispatch reads via these accessors and stamps into
//! [`crate::RenderStats`] for the per-paint trace event.

use crate::Renderer;

impl Renderer {
    /// Microseconds the most recent `draw_buffer_no_present` spent
    /// in the body paint pass. Surfaced as `body_paint_us` in
    /// `event:paint:render_stats`.
    #[must_use]
    pub fn last_body_paint_us(&self) -> u64 {
        self.last_body_paint_us.get()
    }

    /// Microseconds the most recent `draw_buffer_no_present` spent
    /// in the post-body paint pass. Surfaced as `post_body_paint_us`
    /// in `event:paint:render_stats`.
    #[must_use]
    pub fn last_post_body_paint_us(&self) -> u64 {
        self.last_post_body_paint_us.get()
    }

    /// Post-body renderer sub-stage timings from the most recent draw.
    /// Surfaced as `event:renderer_post_body_stages`.
    #[must_use]
    pub fn last_post_body_stages(&self) -> crate::RendererPostBodyStages {
        self.last_post_body_stages.get()
    }

    /// Static chrome command-list path from the most recent draw.
    /// Surfaced as `event:chrome_path`.
    #[must_use]
    pub fn last_chrome_path_stats(&self) -> crate::ChromePathStats {
        self.last_chrome_path_stats.get()
    }

    /// Per-table chrome command-list path from the most recent draw
    /// (P14.1). Surfaced as `event:table_chrome_path` and as the
    /// `chrome_overlay_table_us` field of `event:renderer_draw_stages`.
    #[must_use]
    pub fn last_table_chrome_stats(&self) -> crate::TableChromePathStats {
        self.last_table_chrome_stats.get()
    }

    /// Display-row count painted as a soft "loading" placeholder
    /// during the most recent draw because the realized row range of
    /// the painted [`crate::FrameDisplay`] did not cover the live
    /// viewport. Surfaced as `rows_placeholder` in `event:scroll_path`.
    /// Non-zero only on scroll-tick paints that reused the previous
    /// frame outside its realized window.
    #[must_use]
    pub fn last_scroll_placeholder_rows(&self) -> u32 {
        self.last_scroll_placeholder_rows.get()
    }

    /// Display-row count synchronously realized by the section-10
    /// strip-realize path on the most recent draw. UI stamps via
    /// [`Self::set_last_scroll_strip_rows`] after the dispatch arm
    /// decides whether scroll-tick paint extended the cached frame or
    /// fell through to the placeholder strip.
    #[must_use]
    pub fn last_scroll_strip_rows(&self) -> u32 {
        self.last_scroll_strip_rows.get()
    }

    /// UI-side setter for [`Self::last_scroll_strip_rows`]. Called
    /// after [`crate::Renderer::draw_buffer`] returns from
    /// `window_paint.rs` so the next `event:scroll_path` payload can
    /// report the strip work that just happened. Stays a `&self`
    /// method to match the rest of the renderer's interior-mutability
    /// counters.
    pub fn set_last_scroll_strip_rows(&self, rows: u32) {
        self.last_scroll_strip_rows.set(rows);
    }

    /// Chrome-overlay sub-stage breakdown from the most recent draw.
    /// UI stamps this into [`crate::RenderStats`] so the
    /// `event:renderer_draw_stages` row carries the per-sub-stage
    /// split alongside the existing `chrome_overlay_us` total.
    #[must_use]
    pub fn last_chrome_overlay_breakdown(&self) -> crate::RendererChromeOverlayBreakdown {
        self.last_chrome_overlay_breakdown.get()
    }
}
