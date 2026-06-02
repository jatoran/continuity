//! Tier-1 OS title-bar theming: keep the Windows-drawn caption
//! (`DWMWA_USE_IMMERSIVE_DARK_MODE`) in sync with the active editor
//! theme's light/dark cast.
//!
//! Thread ownership: UI thread (HWND owner). `DwmSetWindowAttribute`
//! must run on the thread that owns the window, which is where both the
//! window-creation path and the paint path run.

use crate::Window;

impl Window {
    /// Match the OS-drawn title bar to the active theme's light/dark
    /// cast.
    ///
    /// Idempotent and cheap: it recomputes the desired dark state each
    /// call (an allocation-free theme lookup) but only issues the
    /// `DwmSetWindowAttribute` syscall when that state differs from the
    /// last value applied. This makes it safe to call on the paint path,
    /// which is the single hook that covers every theme change — each one
    /// already triggers a `theme_apply` invalidate and therefore a paint.
    /// It is also called once at window creation, before the window is
    /// shown, so the very first frame has a correctly-themed caption with
    /// no light/dark flash.
    ///
    /// On Windows builds without the immersive dark-mode attribute
    /// (pre-1809) the syscall fails; we cache the attempt anyway so a
    /// failing call fires at most once per genuine light/dark flip rather
    /// than on every paint.
    pub(crate) fn sync_titlebar_theme(&mut self) {
        if self.hwnd.is_invalid() {
            return;
        }
        let dark = self.active_theme.is_dark();
        if self.titlebar_dark_applied == Some(dark) {
            return;
        }
        // Record the attempt regardless of outcome: on supported builds
        // it succeeded; on unsupported builds there is no OS dark caption
        // to set and retrying every paint would be pure waste.
        let _ = continuity_win::set_titlebar_dark_mode(self.hwnd, dark);
        self.titlebar_dark_applied = Some(dark);
    }
}
