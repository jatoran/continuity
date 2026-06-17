//! View command implementations: scroll (instant + animated), zoom,
//! soft-wrap toggle.
//!
//! These are wired through `command::Context` so the registry-driven
//! command path stays uniform with everything in Phases 4–8. The
//! caret-follows-viewport reveal (`ensure_primary_caret_visible` and
//! friends) lives in the `caret_reveal` submodule; the row-estimate
//! helpers it uses live in `caret_visibility`.

use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::motion::STRUCTURAL_MOTION_MS;
use crate::window::END_OF_BUFFER_BOTTOM_PADDING_DIP;
use crate::window_helpers::invalidate_hwnd;
use crate::{Error, Window};

mod caret_reveal;
mod caret_visibility;
pub(crate) mod geometry_anchor;

impl Window {
    /// Toggle soft wrap. Invalidates layouts for the now-stale wrap width.
    ///
    /// δ.3 — anchored so the caret stays at the same screen y across
    /// the wrap-mode flip. Without this, turning wrap on against a long
    /// line could push the caret off-screen by many rows.
    pub(crate) fn view_toggle_soft_wrap_impl(&mut self) -> Result<(), Error> {
        self.with_caret_line_anchored(|w| {
            w.view.toggle_soft_wrap();
            let new_key = w.view.wrap_width_key();
            w.cache.invalidate_other_wrap_widths(new_key);
        });
        // δ.6 Tier 3 — contract (C) writeback to settings.toml.
        self.persist_toggle_or_log("editor", "word_wrap", self.view.soft_wrap);
        self.request_state_save();
        // The toggle reflows; without an explicit invalidate the only
        // thing scheduling the repaint is the caret-blink timer, so the
        // visual change lags ~500 ms behind the keypress.
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// Pixel-locked scroll by `lines` logical lines (negative = up).
    pub(crate) fn view_scroll_lines_impl(&mut self, lines: f32) -> Result<(), Error> {
        let hwnd = self.hwnd();
        self.stop_scroll_anim(hwnd);
        let line_height = self.effective_line_height();
        let dy = lines * line_height;
        let content_h = self.estimated_content_height();
        self.view.line_height_dip = line_height;
        self.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
        self.view.scroll_instant(dy, content_h);
        self.request_state_save();
        Ok(())
    }

    /// Animated scroll by one viewport-page worth (PageDown / PageUp).
    pub(crate) fn view_scroll_page_impl(&mut self, direction: f32) -> Result<(), Error> {
        self.cancel_scroll_inertia();
        let viewport_h = self.view.viewport_height_dip;
        let line_height = self.effective_line_height();
        // Leave one line of overlap so the user doesn't lose context per
        // page (Sublime / VS Code convention).
        let delta = direction * (viewport_h - line_height).max(line_height);
        let target = self.view.scroll_y_dip + delta;
        let content_h = self.estimated_content_height();
        self.view.line_height_dip = line_height;
        self.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.view.jump_to(target, content_h);
            let hwnd = self.hwnd();
            self.stop_scroll_anim(hwnd);
            self.request_state_save();
            return Ok(());
        }
        let now_ms = unsafe { GetTickCount64() };
        self.view
            .scroll_animated(target, content_h, now_ms, u64::from(STRUCTURAL_MOTION_MS));
        let hwnd = self.hwnd();
        self.start_scroll_anim(hwnd);
        self.request_state_save();
        Ok(())
    }

    /// Animated scroll to the document start.
    pub(crate) fn view_scroll_doc_start_impl(&mut self) -> Result<(), Error> {
        self.cancel_scroll_inertia();
        let content_h = self.estimated_content_height();
        self.view.line_height_dip = self.effective_line_height();
        self.view.overscroll_bottom_dip = self.overscroll_bottom_dip();
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.view.jump_to(0.0, content_h);
            let hwnd = self.hwnd();
            self.stop_scroll_anim(hwnd);
            self.request_state_save();
            return Ok(());
        }
        let now_ms = unsafe { GetTickCount64() };
        self.view
            .scroll_animated(0.0, content_h, now_ms, u64::from(STRUCTURAL_MOTION_MS));
        let hwnd = self.hwnd();
        self.start_scroll_anim(hwnd);
        self.request_state_save();
        Ok(())
    }

    /// Animated scroll to the document end.
    pub(crate) fn view_scroll_doc_end_impl(&mut self) -> Result<(), Error> {
        self.cancel_scroll_inertia();
        let content_h = self.estimated_content_height() + END_OF_BUFFER_BOTTOM_PADDING_DIP;
        // MUST land the last line at the viewport BOTTOM (one EOF inset),
        // never the top: this path passes `content_h` itself as the scroll
        // target, so a non-zero overscroll allowance in the clamp would
        // overshoot upward by that allowance. Zero it for the doc-end snap
        // so Ctrl+End is unaffected by scroll-past-end.
        self.view.overscroll_bottom_dip = 0.0;
        if self.motion_policy().is_reduced_motion() || !self.view_options.smooth_scroll {
            self.view.jump_to(content_h, content_h);
            let hwnd = self.hwnd();
            self.stop_scroll_anim(hwnd);
            self.request_state_save();
            return Ok(());
        }
        let now_ms = unsafe { GetTickCount64() };
        self.view.scroll_animated(
            content_h,
            content_h,
            now_ms,
            u64::from(STRUCTURAL_MOTION_MS),
        );
        let hwnd = self.hwnd();
        self.start_scroll_anim(hwnd);
        self.request_state_save();
        Ok(())
    }
}
