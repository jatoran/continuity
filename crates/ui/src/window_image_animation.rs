//! Image-animation tick (animated GIFs).
//!
//! Drives `Renderer::advance_image_animations` from a per-window
//! `WM_TIMER`. Armed lazily — the timer only fires while the image
//! cache contains at least one animated entry; static-image-only
//! sessions never pay timer cost.
//!
//! Thread ownership: UI thread (owner of [`crate::Window`]).

use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::window_timers::{IMAGE_ANIMATION_TIMER_ID, IMAGE_ANIMATION_TIMER_MS};
use crate::Window;

impl Window {
    /// Arm the image-animation timer if at least one animated image
    /// is resident in the renderer's cache and the timer isn't
    /// already running. Cheap no-op when no animated entries exist
    /// or the timer is already armed.
    ///
    /// Called from [`Self::ensure_image_animation_timer`] after every
    /// paint completes (paint is the moment when the cache may have
    /// just decoded a new animated GIF).
    pub(crate) fn ensure_image_animation_timer(&mut self) {
        if self.view_options.image_animation_timer_active {
            return;
        }
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        if !renderer.has_animated_images() {
            return;
        }
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                IMAGE_ANIMATION_TIMER_ID,
                IMAGE_ANIMATION_TIMER_MS,
                None,
            );
        }
        self.view_options.image_animation_timer_active = true;
    }

    /// `WM_TIMER` handler for [`IMAGE_ANIMATION_TIMER_ID`]. Advances
    /// every animated entry and invalidates the window when at least
    /// one frame changed. Auto-disarms when no animated entries
    /// remain (eviction, device loss, or static-only session).
    pub(crate) fn on_image_animation_tick(&mut self, hwnd: windows::Win32::Foundation::HWND) {
        let Some(renderer) = self.renderer.as_ref() else {
            self.disarm_image_animation_timer();
            return;
        };
        let now_ms = self.now_ms();
        let advanced = renderer.advance_image_animations(now_ms);
        if advanced {
            self.invalidate_with_reason(hwnd, "image_animation");
        }
        if !renderer.has_animated_images() {
            self.disarm_image_animation_timer();
        }
    }

    fn disarm_image_animation_timer(&mut self) {
        if !self.view_options.image_animation_timer_active {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), IMAGE_ANIMATION_TIMER_ID);
        }
        self.view_options.image_animation_timer_active = false;
    }
}
