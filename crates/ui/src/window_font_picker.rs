//! Font-picker command implementations on `Window`: family / size /
//! ligatures + the legacy `ChooseFontW` Win32 dialog used as a fallback
//! when DirectWrite enumeration fails.
//!
//! Thread ownership: UI thread. The Win32 picker (`ChooseFontW`) is a
//! modal dialog; while it's open the message pump stops pumping our
//! window's queue. The DirectWrite-driven palette overlay path is the
//! production code path; the legacy dialog only fires when DirectWrite
//! enumeration returns an empty list.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM};
use windows::Win32::Graphics::Gdi::{
    GetDC, ReleaseDC, CLEARTYPE_QUALITY, DEFAULT_PITCH, FF_DONTCARE, FONT_CHARSET,
    FONT_CLIP_PRECISION, FW_NORMAL, LOGFONTW, OUT_DEFAULT_PRECIS,
};
use windows::Win32::UI::Controls::Dialogs::{
    ChooseFontW, CF_BOTH, CF_FORCEFONTEXIST, CHOOSEFONTW, CHOOSEFONT_FONT_TYPE,
};

use crate::window_helpers::invalidate_hwnd;
use crate::Window;

/// Family selected by [`pick_font_family_via_choose_font`]. `Some(name)`
/// means the user accepted; `None` means cancel or error.
fn pick_font_family_via_choose_font(hwnd: HWND) -> Option<String> {
    // Prepare a LOGFONTW with the current default — `ChooseFontW` mutates
    // it in place to reflect the user's selection.
    let mut logfont = LOGFONTW {
        lfHeight: -14,
        lfWidth: 0,
        lfEscapement: 0,
        lfOrientation: 0,
        lfWeight: FW_NORMAL.0 as i32,
        lfItalic: 0,
        lfUnderline: 0,
        lfStrikeOut: 0,
        lfCharSet: FONT_CHARSET(0),
        lfOutPrecision: OUT_DEFAULT_PRECIS,
        lfClipPrecision: FONT_CLIP_PRECISION(0),
        lfQuality: CLEARTYPE_QUALITY,
        lfPitchAndFamily: DEFAULT_PITCH.0 | FF_DONTCARE.0,
        lfFaceName: face_name_to_array("Cascadia Mono"),
    };
    let mut cf = CHOOSEFONTW {
        lStructSize: u32::try_from(std::mem::size_of::<CHOOSEFONTW>()).unwrap_or(0),
        hwndOwner: hwnd,
        hDC: unsafe { GetDC(Some(hwnd)) },
        lpLogFont: std::ptr::addr_of_mut!(logfont),
        iPointSize: 0,
        Flags: CF_BOTH | CF_FORCEFONTEXIST,
        rgbColors: COLORREF(0),
        lCustData: LPARAM(0),
        lpfnHook: None,
        lpTemplateName: PCWSTR::null(),
        hInstance: Default::default(),
        lpszStyle: windows::core::PWSTR::null(),
        nFontType: CHOOSEFONT_FONT_TYPE(0),
        ___MISSING_ALIGNMENT__: 0,
        nSizeMin: 6,
        nSizeMax: 96,
    };

    let ok: BOOL = unsafe { ChooseFontW(&mut cf) };
    unsafe {
        ReleaseDC(Some(hwnd), cf.hDC);
    }
    if !ok.as_bool() {
        return None;
    }
    Some(face_name_from_array(&logfont.lfFaceName))
}

fn face_name_to_array(s: &str) -> [u16; 32] {
    let mut buf = [0u16; 32];
    let wide: Vec<u16> = std::ffi::OsStr::new(s).encode_wide().collect();
    for (i, ch) in wide.iter().take(31).enumerate() {
        buf[i] = *ch;
    }
    buf
}

fn face_name_from_array(arr: &[u16; 32]) -> String {
    let end = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    OsString::from_wide(&arr[..end])
        .to_string_lossy()
        .into_owned()
}

impl Window {
    /// §E3 — open the font-picker palette overlay. Lists every
    /// installed Windows font family (DirectWrite system collection) and
    /// previews each one live as the highlight moves. Enter keeps the
    /// previewed family; Esc reverts to the family in effect when the
    /// picker opened.
    ///
    /// On a headless / null HWND (test harness, early init) DirectWrite
    /// enumeration is still safe — the factory is created independently
    /// of the window. The legacy Win32 `ChooseFontW` path is kept around
    /// for the (rare) case where DirectWrite enumeration fails or no
    /// fonts are installed.
    pub(crate) fn pick_font_family_impl(&mut self) -> Result<(), crate::Error> {
        let families =
            match continuity_layout::DWriteFactory::new().and_then(|f| f.system_font_families()) {
                Ok(list) if !list.is_empty() => list,
                _ => {
                    // Fallback: legacy Win32 ChooseFontW dialog. The
                    // modal dialog is single-shot, so any returned
                    // family is a commit (not a preview) — route through
                    // the deferred-swap path so the body doesn't flash
                    // overflow against old wrap break points. See
                    // `window_font_swap`.
                    if let Some(family) = pick_font_family_via_choose_font(self.hwnd) {
                        self.request_font_change(Some(family.clone()), None);
                        self.persist_string_or_log("editor", "font_family_prose", &family);
                    }
                    return Ok(());
                }
            };
        let original = self.prose_font_family.clone();
        self.overlays.open_font_picker(families, original);
        self.focus_overlay_input();
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn set_font_size_impl(&mut self, size_dip: f32) -> Result<(), crate::Error> {
        let size = size_dip.clamp(6.0, 96.0);
        // Deferred-swap commit: request the size change through the
        // pending-font pipeline so the projection worker rebuilds wrap
        // break points for the new glyph advance in the background, and
        // body paint keeps using the previous size until the matching
        // display map lands. See `window_font_swap`.
        self.request_font_change(None, Some(size));
        // Contract (C): commit the new base size to settings.toml so
        // the pick survives relaunch.
        self.persist_float_or_log("editor", "font_size", f64::from(size));
        Ok(())
    }

    pub(crate) fn set_font_family(&mut self, family: String) {
        let family = family.trim().to_string();
        if family.is_empty() {
            return;
        }
        // FontStateId hashes the family — invalidate other-font-state layouts.
        let scaled_size = self.scaled_font_size();
        let next_state = continuity_layout::FontStateId::from_parts(
            &family,
            scaled_size,
            super::window::FONT_LOCALE,
            self.dpi_scale(),
        );
        if next_state != self.font_state {
            self.cache.invalidate_other_font_states(next_state);
        }
        self.prose_font_family = family;
        self.text_format = None;
        invalidate_hwnd(self.hwnd);
    }

    pub(crate) fn toggle_ligatures_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.ligatures = !self.view_options.ligatures;
        self.persist_toggle_or_log("editor", "ligatures", self.view_options.ligatures);
        self.text_format = None; // Re-create with updated typography flag.
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    pub(crate) fn scaled_font_size(&self) -> f32 {
        let base = self
            .font_size_dip_override
            .unwrap_or(super::window::FONT_SIZE_DIP);
        base * self.view.font_size_scale
    }

    /// Per-frame row stride in DIPs: the zoom-scaled font size times the
    /// configured `[editor].line_height` multiplier, rounded to whole
    /// DIPs so rows land on pixel boundaries.
    ///
    /// This is the canonical line height for *all* vertical geometry —
    /// paint stride, scroll math, hit-testing, caret anchoring, content
    /// height, image-row reservations. It replaces the former fixed
    /// [`crate::window::LINE_HEIGHT_DIP`] constant everywhere geometry is
    /// computed, so Ctrl+wheel zoom scales row height in lock-step with
    /// glyph size and rows never overlap. `LINE_HEIGHT_DIP` remains the
    /// zoom-1 / multiplier-1 reference value.
    pub(crate) fn effective_line_height(&self) -> f32 {
        (self.scaled_font_size() * self.settings_projections.line_height_multiplier)
            .round()
            .max(1.0)
    }
}
