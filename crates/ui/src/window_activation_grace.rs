//! Post-activation grace window — the UI thread blocks nonessential
//! deferred work (spell recheck, MRU display-map prewarm builds,
//! spectator-pane decoration submission, outline rebuilds against a
//! stale revision) for [`Window::ACTIVATION_GRACE_MS`] after
//! `WM_ACTIVATEAPP(true)` so input + paint dominate the moment the
//! user returns to the window.
//!
//! **Thread ownership**: `last_activation_tick` is read and written
//! only on the window's UI thread.

use crate::window::Window;

impl Window {
    /// Grace window after `WM_ACTIVATEAPP(true)` during which the
    /// UI thread blocks all nonessential deferred work — spell
    /// recheck, MRU display-map prewarm builds, spectator-pane
    /// decoration submission, outline rebuilds against a stale
    /// revision. Input + paint dominate during this window; the
    /// deferred work resumes once the grace expires and the user
    /// has been idle long enough for the existing prewarm gate.
    pub(crate) const ACTIVATION_GRACE_MS: u64 = 1_000;

    /// True when we are inside the post-activation grace window
    /// (recent `WM_ACTIVATEAPP(true)`). Cheap branch — call from
    /// any idle/deferred path before doing whole-buffer work.
    #[inline]
    pub(crate) fn in_activation_grace(&self) -> bool {
        if self.last_activation_tick == 0 {
            return false;
        }
        let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        now.saturating_sub(self.last_activation_tick) < Self::ACTIVATION_GRACE_MS
    }
}
