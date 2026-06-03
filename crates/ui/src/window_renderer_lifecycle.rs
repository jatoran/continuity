//! Per-`WM_SIZE` client-pixel refresh, lazy renderer construction, and
//! DirectWrite text-format/font-state (re)initialization. All three
//! belong together because they share the same trigger — the first
//! paint after a window resize or font change — and they collectively
//! own the renderer + `text_format` slots on `Window`.
//!
//! **Thread ownership**: the window's UI thread.

use continuity_render::Renderer;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::window::{Window, FONT_LOCALE};
use crate::window_resize_fence::{ClientResizeDelta, ClientSize};
use crate::Error;

impl Window {
    pub(crate) fn refresh_client_size(&mut self, hwnd: HWND) {
        let mut rect = RECT::default();
        unsafe {
            if GetClientRect(hwnd, &mut rect).is_ok() {
                let new_w = (rect.right - rect.left).max(1) as u32;
                let new_h = (rect.bottom - rect.top).max(1) as u32;
                let client_size_changed = new_w != self.client_width || new_h != self.client_height;
                let renderer_target_mismatch = self
                    .renderer
                    .as_ref()
                    .is_some_and(|renderer| renderer.back_buffer_size() != (new_w, new_h));
                if client_size_changed || renderer_target_mismatch {
                    let old_w = self.client_width;
                    let old_h = self.client_height;
                    let old_viewport_w = self.view.viewport_width_dip;
                    let old_viewport_h = self.view.viewport_height_dip;
                    let old_wrap_width = self.view.wrap_width_key();
                    let resize_delta = ClientResizeDelta::from_clients(
                        ClientSize::new(old_w, old_h),
                        ClientSize::new(new_w, new_h),
                    );
                    if client_size_changed {
                        self.client_width = new_w;
                        self.client_height = new_h;
                        // Phase 13: focused pane's body rect = its outer rect
                        // minus the tab strip. The viewport reflects the body.
                        //
                        // During a live Win32 sizing loop (WM_ENTERSIZEMOVE
                        // → WM_EXITSIZEMOVE) every WM_SIZE tick takes the
                        // cheap unanchored path — the per-tick caret
                        // anchor build does a fresh FrameDisplay projection
                        // and that dominates resize CPU. A single anchor
                        // captured at WM_ENTERSIZEMOVE is restored once at
                        // WM_EXITSIZEMOVE so the caret-line screen-y
                        // contract still holds for the final frame.
                        if self.is_live_resizing {
                            self.refresh_focused_viewport_unanchored();
                            self.resize_changed = true;
                        } else {
                            self.refresh_focused_viewport();
                        }
                    }
                    // Reuse the existing D3D/D2D/DirectWrite stack — only
                    // the swap-chain back buffer and bound D2D target
                    // need to be recreated for the new client size. The
                    // swap chain uses DXGI_SCALING_STRETCH, so DWM can
                    // stretch the last presented frame across the short gap
                    // between WM_SIZE and the next paint instead of showing
                    // uncovered right/bottom strips. If the cheap-path resize
                    // fails (e.g. device removed), drop the renderer so the
                    // next paint goes through the cold construction path and
                    // surfaces the error there.
                    let mut renderer_resize = "renderer_absent";
                    if let Some(renderer) = self.renderer.as_mut() {
                        let current_target = renderer.back_buffer_size();
                        let can_defer_shrink = self.is_live_resizing
                            && resize_delta.has_shrink_axis()
                            && new_w <= current_target.0
                            && new_h <= current_target.1;
                        if can_defer_shrink {
                            self.deferred_renderer_resize = Some((new_w.max(1), new_h.max(1)));
                            renderer_resize = "deferred_shrink";
                            if crate::paint_trace::is_trace_enabled() {
                                crate::paint_trace::log_event(
                                    "live_resize_renderer_resize_deferred",
                                    &format!(
                                        concat!(
                                            "old_client={}x{} new_client={}x{} ",
                                            "delta={} renderer_target={}x{}"
                                        ),
                                        old_w,
                                        old_h,
                                        new_w,
                                        new_h,
                                        resize_delta.as_trace_label(),
                                        current_target.0,
                                        current_target.1,
                                    ),
                                );
                            }
                        } else {
                            let resize_label = if renderer_target_mismatch && !client_size_changed {
                                "ok_reconcile"
                            } else {
                                "ok"
                            };
                            if renderer
                                .resize_for_hwnd(hwnd, new_w.max(1), new_h.max(1))
                                .is_err()
                            {
                                self.renderer = None;
                                self.deferred_renderer_resize = None;
                                renderer_resize = "error_dropped";
                            } else {
                                self.deferred_renderer_resize = None;
                                renderer_resize = resize_label;
                            }
                        }
                    }
                    if crate::paint_trace::is_trace_enabled() {
                        let new_wrap_width = self.view.wrap_width_key();
                        let renderer_target = self
                            .renderer
                            .as_ref()
                            .map(|renderer| renderer.back_buffer_size())
                            .unwrap_or((0, 0));
                        crate::paint_trace::log_event(
                            "resize_projection_inputs",
                            &format!(
                                concat!(
                                    "old_client={}x{} new_client={}x{} ",
                                    "renderer_target={}x{} ",
                                    "old_viewport={:.1}x{:.1} new_viewport={:.1}x{:.1} ",
                                    "old_wrap={} new_wrap={} wrap_changed={} ",
                                    "live_resizing={} renderer_resize={}"
                                ),
                                old_w,
                                old_h,
                                new_w,
                                new_h,
                                renderer_target.0,
                                renderer_target.1,
                                old_viewport_w,
                                old_viewport_h,
                                self.view.viewport_width_dip,
                                self.view.viewport_height_dip,
                                old_wrap_width,
                                new_wrap_width,
                                old_wrap_width != new_wrap_width,
                                self.is_live_resizing,
                                renderer_resize,
                            ),
                        );
                    }
                    // Schedule a repaint. The window class is not
                    // registered with CS_HREDRAW / CS_VREDRAW, and
                    // DefWindowProc only invalidates newly exposed
                    // regions on grow — shrinking the client area
                    // leaves the existing pixels intact and the
                    // soft-wrap projection stays stale until the next
                    // unrelated invalidate. Force a full client
                    // invalidate (no erase) on every size delta so
                    // both grow and shrink motions re-project against
                    // the new viewport width during the drag.
                    self.invalidate(hwnd);
                    // Layout-cache note: the focused pane keys its
                    // soft-wrap entries with `wrap_width_dip = 0` (see
                    // `crates/render/src/wrap_paint.rs`), so a focused-
                    // viewport resize does NOT invalidate any focused
                    // entries. Spectator panes use a real wrap width
                    // key, but their viewports are derived from the
                    // pane tree — a focused-pane size change does not
                    // shift any spectator key. Stale spectator entries
                    // age out through the LRU bound; eagerly purging
                    // them on every WM_SIZE tick would discard the
                    // exact entries the next paint needs.
                }
            }
        }
    }

    pub(crate) fn ensure_renderer(&mut self, hwnd: HWND) -> Result<(), Error> {
        if self.client_width == 0 || self.client_height == 0 {
            self.refresh_client_size(hwnd);
        }
        if self.renderer.is_none() {
            let renderer =
                Renderer::for_hwnd(hwnd, self.client_width.max(1), self.client_height.max(1))?;
            // F5 Pass 2 fix: `apply_settings` runs during `Window::new`
            // BEFORE the renderer is lazily created, so its
            // `set_image_cache_capacity` call was a no-op. Push the
            // resolved target here too so the cache activates on the
            // very first paint (otherwise the cache stays at 0 and
            // `get_or_decode` returns `Ok(None)`, which is why inline
            // images rendered as the raw `![](url)` text instead of
            // the decoded bitmap).
            renderer.set_image_cache_capacity(self.image_cache_bytes_target);
            self.renderer = Some(renderer);
        }
        if self.text_format.is_none() {
            let scaled_size = self.scaled_font_size();
            self.text_format = Some(self.dwrite.text_format(
                &self.prose_font_family,
                scaled_size,
                FONT_LOCALE,
            )?);
            self.apply_tab_stop_to_text_format();
            self.font_state = self.current_font_state_id();
        }
        Ok(())
    }

    /// Pin the body text format's DirectWrite incremental tab stop to
    /// `[editor].tab_width` columns so a literal `\t` renders at the
    /// configured width — for both the renderer (`params.format`) and
    /// the projection worker, which share this one COM handle. Applied
    /// at every format (re)build so the worker's wrap measurement and
    /// the painted glyph agree from the first frame after a font / tab
    /// change. A `tab_width` of `0` leaves the font's default tab stop
    /// in effect (pre-settings behaviour). No-op when no format exists.
    pub(crate) fn apply_tab_stop_to_text_format(&self) {
        let tab_width = self.view_options.tab_width;
        if tab_width == 0 {
            return;
        }
        let Some(format) = self.text_format.as_ref() else {
            return;
        };
        let space_advance = continuity_render::text_metrics::measure_space_advance_dip(
            self.dwrite.raw(),
            format,
            self.scaled_font_size(),
        );
        let _ = unsafe { format.SetIncrementalTabStop(space_advance * tab_width as f32) };
    }

    pub(crate) fn apply_deferred_renderer_resize(&mut self, hwnd: HWND) -> Result<(), Error> {
        let Some((width, height)) = self.deferred_renderer_resize.take() else {
            return Ok(());
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return Ok(());
        };
        let old_target = renderer.back_buffer_size();
        let tracing = crate::paint_trace::is_trace_enabled();
        let started = tracing.then(std::time::Instant::now);
        let result = renderer.resize_for_hwnd(hwnd, width, height);
        let elapsed_us = started
            .map(|started| u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        let new_target = renderer.back_buffer_size();

        if tracing {
            let (result_label, hresult) = match &result {
                Ok(()) => ("ok", 0),
                Err(continuity_render::Error::Graphics(error)) => ("err", error.code().0 as u32),
                Err(continuity_render::Error::Win(_) | continuity_render::Error::Layout(_)) => {
                    ("err", 0)
                }
            };
            crate::paint_trace::log_event(
                "live_resize_renderer_resize_apply",
                &format!(
                    concat!(
                        "result={} hr=0x{:08X} elapsed_us={} ",
                        "client={}x{} requested={}x{} ",
                        "old_renderer_target={}x{} new_renderer_target={}x{}"
                    ),
                    result_label,
                    hresult,
                    elapsed_us,
                    self.client_width,
                    self.client_height,
                    width,
                    height,
                    old_target.0,
                    old_target.1,
                    new_target.0,
                    new_target.1,
                ),
            );
        }

        if result.is_err() {
            self.renderer = None;
        }
        result.map_err(Error::from)
    }

    /// Force a font/text-format rebuild on next paint and evict stale
    /// cache entries for the previous font state.
    ///
    /// δ.3 — callers that mutate `view.font_size_scale` or
    /// `prose_font_family` immediately before invoking this method must
    /// wrap their *whole* mutation (mutation + invalidation) in
    /// [`Self::with_caret_line_anchored`], since the anchor must be
    /// captured before any font-state change. See the call sites in
    /// `window_view`, `window_runtime`, and `window_settings_reload`.
    pub(crate) fn invalidate_font_state(&mut self) {
        self.text_format = None;
        // Drop layouts that were built against any other font_state. We
        // recompute `font_state` next paint and the LRU bound trims the rest.
        let next = self.current_font_state_id();
        if next != self.font_state {
            self.cache.invalidate_other_font_states(next);
        }
    }
}
