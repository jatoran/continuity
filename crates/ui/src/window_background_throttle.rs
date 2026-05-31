//! β — background-window vsync drop.
//!
//! Two conditions skip the compositing pipeline early in `on_paint`:
//!   1. **Minimized.** No surface is visible; running the full paint
//!      pipeline wastes CPU and battery. WM_PAINT may still fire for
//!      the icon-strip / Aero peek — `DefWindowProc` handles that.
//!   2. **Unfocused.** Throttle to ~30 Hz by skipping every other
//!      paint. Perceptually invisible: the user isn't looking at this
//!      window, and the next InvalidateRect/WM_PAINT round-trip picks
//!      up where we left off.
//!
//! Flags are toggled by the WM_SIZE (`SIZE_MINIMIZED`) and
//! WM_ACTIVATEAPP handlers in `window_dispatch`.
//!
//! Thread ownership: UI thread of one window.

use crate::Window;

impl Window {
    /// Pure decision: should the current `on_paint` call return early?
    /// Bumps `background_paint_tick` as a side effect so the unfocused
    /// frame-skip alternates deterministically.
    pub(crate) fn should_skip_background_paint(&mut self) -> bool {
        if self.is_window_minimized {
            return true;
        }
        self.background_paint_tick = self.background_paint_tick.wrapping_add(1);
        if !self.is_window_focused && self.background_paint_tick.is_multiple_of(2) {
            return true;
        }
        false
    }
}
