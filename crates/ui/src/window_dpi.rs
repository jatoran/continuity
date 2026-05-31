//! Per-window DPI state and `WM_DPICHANGED` reflow.
//!
//! Thread ownership: all state touched here (`window_dpi`, client size,
//! renderer target, text format, font key, and viewport) is owned by the
//! window's UI thread.

use std::time::Instant;

use continuity_layout::FontStateId;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

use crate::window::{Window, FONT_LOCALE};
use crate::Error;

const DEFAULT_DPI: u32 = 96;

impl Window {
    /// Current window DPI scale relative to 96-DPI DIPs.
    #[must_use]
    pub(crate) fn dpi_scale(&self) -> f32 {
        (self.window_dpi.max(1) as f32 / DEFAULT_DPI as f32).max(0.01)
    }

    /// Client width in DIPs.
    #[must_use]
    pub(crate) fn client_width_dip(&self) -> f32 {
        self.client_width.max(1) as f32 / self.dpi_scale()
    }

    /// Client height in DIPs.
    #[must_use]
    pub(crate) fn client_height_dip(&self) -> f32 {
        self.client_height.max(1) as f32 / self.dpi_scale()
    }

    /// Convert a physical-pixel client point from Win32 into DIPs.
    #[must_use]
    pub(crate) fn physical_point_to_dip(&self, x: i32, y: i32) -> (i32, i32) {
        let scale = self.dpi_scale();
        (
            (x as f32 / scale).round() as i32,
            (y as f32 / scale).round() as i32,
        )
    }

    /// Font-state key for the current font family, zoom, locale, and DPI.
    #[must_use]
    pub(crate) fn current_font_state_id(&self) -> FontStateId {
        FontStateId::from_parts(
            &self.prose_font_family,
            self.scaled_font_size(),
            FONT_LOCALE,
            self.dpi_scale(),
        )
    }

    /// Handle `WM_DPICHANGED`: accept Windows' suggested window rect, then
    /// run every DPI-sensitive mutation inside one caret-line anchor.
    pub(crate) fn handle_dpi_changed(
        &mut self,
        hwnd: HWND,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<(), Error> {
        let old_dpi = self.window_dpi.max(DEFAULT_DPI);
        let new_dpi =
            dpi_from_wparam(wparam).unwrap_or_else(|| continuity_win::dpi_for_window(hwnd));
        let suggested_rect = suggested_rect_from_lparam(lparam);
        let (suggested_rect_w, suggested_rect_h) = suggested_rect_size(suggested_rect.as_ref());

        self.cancel_scroll_inertia();
        self.is_applying_dpi_change = true;
        let set_window_pos_result = if let Some(rect) = suggested_rect.as_ref() {
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    (rect.right - rect.left).max(1),
                    (rect.bottom - rect.top).max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            }
        } else {
            Ok(())
        };
        self.is_applying_dpi_change = false;
        set_window_pos_result?;

        let trace_start = crate::paint_trace::is_trace_enabled().then(Instant::now);
        let reflow_result = self.with_caret_line_anchored(|w| w.apply_dpi_reflow(hwnd, new_dpi));
        if let Some(started) = trace_start {
            crate::paint_trace::log_event(
                "dpi_changed",
                &format!(
                    "old_dpi={old_dpi} new_dpi={new_dpi} suggested_rect_w={suggested_rect_w} \
                     suggested_rect_h={suggested_rect_h} reflow_us={}",
                    started.elapsed().as_micros(),
                ),
            );
        }
        reflow_result?;
        self.invalidate_with_reason(hwnd, "dpi_changed");
        if self.inited {
            self.request_state_save();
        }
        Ok(())
    }

    fn apply_dpi_reflow(&mut self, hwnd: HWND, new_dpi: u32) -> Result<(), Error> {
        self.window_dpi = new_dpi.max(DEFAULT_DPI);
        self.refresh_client_size_for_dpi_change(hwnd);
        self.rebuild_text_format_for_current_dpi()?;
        Ok(())
    }

    fn rebuild_text_format_for_current_dpi(&mut self) -> Result<(), Error> {
        let scaled_size = self.scaled_font_size();
        let format = self
            .dwrite
            .text_format(&self.prose_font_family, scaled_size, FONT_LOCALE)?;
        let next = self.current_font_state_id();
        if next != self.font_state {
            self.cache.invalidate_other_font_states(next);
        }
        self.text_format = Some(format);
        self.font_state = next;
        Ok(())
    }

    fn refresh_client_size_for_dpi_change(&mut self, hwnd: HWND) {
        let mut rect = RECT::default();
        unsafe {
            if windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect).is_err() {
                return;
            }
        }
        let new_w = (rect.right - rect.left).max(1) as u32;
        let new_h = (rect.bottom - rect.top).max(1) as u32;
        let old_w = self.client_width;
        let old_h = self.client_height;
        let old_viewport_w = self.view.viewport_width_dip;
        let old_viewport_h = self.view.viewport_height_dip;
        let old_wrap_width = self.view.wrap_width_key();

        self.client_width = new_w;
        self.client_height = new_h;
        self.refresh_focused_viewport_unanchored();

        let mut renderer_resize = "renderer_absent";
        if let Some(renderer) = self.renderer.as_mut() {
            if renderer
                .resize_for_hwnd(hwnd, new_w.max(1), new_h.max(1))
                .is_err()
            {
                self.renderer = None;
                renderer_resize = "error_dropped";
            } else {
                renderer_resize = "ok";
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
                        "live_resizing={} renderer_resize={} reason=dpi_changed dpi={}"
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
                    self.window_dpi,
                ),
            );
        }
    }
}

fn dpi_from_wparam(wparam: WPARAM) -> Option<u32> {
    let dpi_x = (wparam.0 & 0xffff) as u32;
    let dpi_y = ((wparam.0 >> 16) & 0xffff) as u32;
    let dpi = dpi_x.max(dpi_y);
    (dpi > 0).then_some(dpi)
}

fn suggested_rect_from_lparam(lparam: LPARAM) -> Option<RECT> {
    let ptr = lparam.0 as *const RECT;
    if ptr.is_null() {
        return None;
    }
    let rect = unsafe { &*ptr };
    Some(RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    })
}

fn suggested_rect_size(rect: Option<&RECT>) -> (i32, i32) {
    rect.map(|rect| {
        (
            (rect.right - rect.left).max(0),
            (rect.bottom - rect.top).max(0),
        )
    })
    .unwrap_or((0, 0))
}
