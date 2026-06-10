//! Tier-1 OS title-bar theming: keep the Windows-drawn caption
//! (`DWMWA_USE_IMMERSIVE_DARK_MODE`) in sync with the active editor
//! theme's light/dark cast.
//!
//! Thread ownership: UI thread (HWND owner). `DwmSetWindowAttribute`
//! must run on the thread that owns the window, which is where both the
//! window-creation path and the paint path run.

use windows::core::HSTRING;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageW, SetWindowTextW, ICON_BIG, ICON_SMALL, WM_SETICON,
};

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

    /// Match the OS window caption to the active tab's label so the
    /// taskbar / Alt-Tab entry names the note the user is editing
    /// instead of the application ("continuity").
    ///
    /// Idempotent and cheap like [`Self::sync_titlebar_theme`]: the
    /// label is recomputed each call but `SetWindowTextW` only fires
    /// when it differs from the last applied caption. Safe on the paint
    /// path — every label-changing event (tab switch, first-line edit,
    /// file association, tab rename) already invalidates and therefore
    /// paints.
    pub(crate) fn sync_window_title(&mut self) {
        if self.hwnd.is_invalid() {
            return;
        }
        let label = self
            .tree
            .active_tab()
            .map(|tab| self.tab_label(tab))
            .unwrap_or_default();
        let title = if label.trim().is_empty() {
            "Untitled".to_string()
        } else {
            label
        };
        if self.window_title_applied.as_deref() == Some(title.as_str()) {
            return;
        }
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title.as_str()));
        }
        self.window_title_applied = Some(title);
    }

    /// Attach the embedded application icon to this window's caption and
    /// Alt-Tab entry.
    ///
    /// Sets both the big icon (`ICON_BIG`: Alt-Tab and the large caption
    /// metric) and the small icon (`ICON_SMALL`: the top-left caption
    /// glyph) via per-window `WM_SETICON`. This is surgical — it touches
    /// only this HWND and leaves the shared window class (and therefore the
    /// tab-drag ghost and hidden smoke window) icon-less.
    ///
    /// Thread ownership: UI thread (HWND owner). Called once at window
    /// creation, before the window is shown, so the first frame already
    /// carries the caption icon with no default-icon flash.
    ///
    /// The icons come from [`continuity_win::load_app_icon`], which loads
    /// with `LR_SHARED`, so the OS owns the handle lifetime and there is no
    /// `DestroyIcon` to call. On hosts whose binary has no embedded id-1
    /// icon resource (e.g. test harnesses) the load fails and this method
    /// no-ops — the same tolerance `sync_titlebar_theme` applies to
    /// unsupported caption attributes.
    pub(crate) fn apply_window_icon(&self) {
        if self.hwnd.is_invalid() {
            return;
        }
        if let Ok(icon_big) = continuity_win::load_app_icon(true) {
            unsafe {
                SendMessageW(
                    self.hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(icon_big.0 as isize)),
                );
            }
        }
        if let Ok(icon_small) = continuity_win::load_app_icon(false) {
            unsafe {
                SendMessageW(
                    self.hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_SMALL as usize)),
                    Some(LPARAM(icon_small.0 as isize)),
                );
            }
        }
    }
}
