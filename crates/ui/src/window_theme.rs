//! Per-window active-theme state and conversion to the renderer's color
//! structs.
//!
//! Thread ownership: every `ActiveTheme` is owned by its window's UI thread.
//! The theme TOML is plain data and is `Clone`able, so a worker (e.g. the
//! Phase-12 file watcher) can hand a parsed `Theme` to the UI thread via a
//! channel without any shared-mutable state.
//!
//! Layer note: this module is the only place a `theme::Theme` is converted
//! into a `render::Rgba`. Both crates stay independent of each other.

use continuity_render::{EditorColors, MarkdownColors, Rgba};
use continuity_theme::{
    assets::{bundled_set, neutral_fallback},
    Color, Mode, Theme, ThemeSet,
};
use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

use crate::Error;

/// Per-window theme state: the user's mode preference, an `installed`
/// dark+light pair (defaults to the bundled set), the OS `system_dark`
/// flag, and the resolved current theme.
///
/// Owner: UI thread of one window. Never `Send`-shared mutably across
/// threads; clone the `Theme` if you need a snapshot in another worker.
#[derive(Debug, Clone)]
pub struct ActiveTheme {
    /// User-chosen mode (`dark` / `light` / `system`).
    pub mode: Mode,
    /// `true` when the OS is currently set to a dark color scheme.
    pub system_dark: bool,
    /// The two themes the user has installed (or the bundled defaults).
    pub set: ThemeSet,
    /// The theme picked from `set` according to `mode` and `system_dark`.
    pub current: Theme,
}

impl ActiveTheme {
    /// Build with the bundled `deep_minimal` + `paper` set, sampling the OS
    /// dark-mode flag once.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Theme`] only when the bundled themes themselves
    /// fail to validate (canary tests in `continuity_theme::assets` cover
    /// this — production code can `expect`).
    pub fn bundled() -> Result<Self, Error> {
        let set = bundled_set().map_err(Error::Theme)?;
        let mode = Mode::default();
        let system_dark = read_system_dark();
        let current = set.active(mode, system_dark).clone();
        Ok(Self {
            mode,
            system_dark,
            set,
            current,
        })
    }

    /// Last-resort fallback: hard-coded neutral palette, used when even the
    /// bundled themes fail to load (should be impossible in production —
    /// this is the bottom of [`continuity_theme::assets::resolve_active`]).
    #[must_use]
    pub fn neutral() -> Self {
        let neutral = neutral_fallback();
        let set = ThemeSet {
            dark: neutral.clone(),
            light: neutral.clone(),
        };
        Self {
            mode: Mode::System,
            system_dark: read_system_dark(),
            set,
            current: neutral,
        }
    }

    /// Replace the installed `dark` + `light` pair (e.g. a user-installed
    /// theme TOML loaded by Phase-12). Re-resolves the current theme.
    pub(crate) fn set_installed(&mut self, set: ThemeSet) {
        self.set = set;
        self.recompute_current();
    }

    /// Set the user's mode preference and re-resolve.
    pub(crate) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.recompute_current();
    }

    /// Set the OS `system_dark` flag (called from the
    /// `WM_SETTINGCHANGE` / `ImmersiveColorSet` handler) and re-resolve.
    /// Returns `true` if the active theme actually changed.
    pub(crate) fn set_system_dark(&mut self, dark: bool) -> bool {
        if self.system_dark == dark {
            return false;
        }
        self.system_dark = dark;
        // Only the system mode is sensitive to the OS flag.
        if matches!(self.mode, Mode::System) {
            self.recompute_current();
            return true;
        }
        false
    }

    /// Cycle through `dark` → `light` → `system` → `dark` … and re-resolve.
    /// Returns the new mode.
    pub(crate) fn cycle_mode(&mut self) -> Mode {
        let next = match self.mode {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::System,
            Mode::System => Mode::Dark,
        };
        self.set_mode(next);
        next
    }

    fn recompute_current(&mut self) {
        self.current = self.set.active(self.mode, self.system_dark).clone();
    }

    /// Project the current theme into the renderer's `EditorColors` struct.
    #[must_use]
    pub(crate) fn editor_colors(&self) -> EditorColors {
        editor_colors_from(&self.current)
    }

    /// Project the current theme into the renderer's `MarkdownColors` struct.
    #[must_use]
    pub fn markdown_colors(&self) -> MarkdownColors {
        markdown_colors_from(&self.current)
    }

    /// Fingerprint the active theme content for retained render caches.
    /// The value only needs to be stable within a process; theme loads
    /// and OS dark-mode flips recompute it from the resolved palette.
    #[must_use]
    pub(crate) fn revision_key(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        let mode_tag = match self.mode {
            Mode::Dark => 0u8,
            Mode::Light => 1,
            Mode::System => 2,
        };
        mode_tag.hash(&mut hasher);
        self.system_dark.hash(&mut hasher);
        self.current.name.hash(&mut hasher);
        for (key, color) in &self.current.colors {
            key.hash(&mut hasher);
            color.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// §H1 `editor.foreground_dim` forwarder — used by `window_paint`
    /// to compute the focus-mode dim color.
    #[must_use]
    pub(crate) fn editor_foreground_dim(&self) -> Color {
        self.current.editor_foreground_dim()
    }

    /// §H1 `editor.focus_dim_alpha` forwarder — only the `.a` byte is
    /// consumed by `window_paint`.
    #[must_use]
    pub(crate) fn editor_focus_dim_alpha(&self) -> Color {
        self.current.editor_focus_dim_alpha()
    }
}

/// Convert a `theme::Color` (sRGB 0-255) to a `render::Rgba` (linear-ish
/// 0..=1). Matches the existing `D2D1_COLOR_F` semantics — D2D treats
/// these as sRGB on the wire and applies the gamma at composition time.
#[must_use]
pub(crate) fn rgba_from_color(c: Color) -> Rgba {
    Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: f32::from(c.a) / 255.0,
    }
}

/// Build an [`EditorColors`] from a fully validated [`Theme`].
#[must_use]
pub(crate) fn editor_colors_from(t: &Theme) -> EditorColors {
    EditorColors {
        bg: rgba_from_color(t.editor_background()),
        fg: rgba_from_color(t.editor_foreground()),
        caret: rgba_from_color(t.editor_cursor_primary()),
        secondary_caret: rgba_from_color(t.editor_cursor_secondary()),
        selection: rgba_from_color(t.editor_selection()),
        selection_inactive: rgba_from_color(t.editor_selection_inactive()),
        line_highlight: rgba_from_color(t.editor_line_highlight()),
        line_number: rgba_from_color(t.editor_line_number()),
        line_number_active: rgba_from_color(t.editor_line_number_active()),
        indent_guide: rgba_from_color(t.editor_indent_guide()),
        indent_guide_active: rgba_from_color(t.editor_indent_guide_active()),
        search_match: rgba_from_color(t.editor_search_match()),
        search_match_active: rgba_from_color(t.editor_search_match_active()),
        find_bar_bg: rgba_from_color(t.editor_find_bar_background()),
        search_minimap_bg: rgba_from_color(t.editor_search_minimap_background()),
        search_minimap_match: rgba_from_color(t.editor_search_minimap_match()),
        search_minimap_match_active: rgba_from_color(t.editor_search_minimap_match_active()),
        minimap_bg: rgba_from_color(t.editor_minimap_background()),
        minimap_fg: rgba_from_color(t.editor_minimap_foreground()),
        minimap_viewport_indicator: rgba_from_color(t.editor_minimap_viewport_indicator()),
        loading_overlay_bg: rgba_from_color(t.editor_loading_overlay_background()),
        loading_overlay_fg: rgba_from_color(t.editor_loading_overlay_foreground()),
        loading_overlay_border: rgba_from_color(t.editor_loading_overlay_border()),
    }
}

/// Build a [`MarkdownColors`] from a fully validated [`Theme`].
#[must_use]
pub(crate) fn markdown_colors_from(t: &Theme) -> MarkdownColors {
    MarkdownColors {
        heading: [
            rgba_from_color(t.markdown_heading(1)),
            rgba_from_color(t.markdown_heading(2)),
            rgba_from_color(t.markdown_heading(3)),
            rgba_from_color(t.markdown_heading(4)),
            rgba_from_color(t.markdown_heading(5)),
            rgba_from_color(t.markdown_heading(6)),
        ],
        bold: rgba_from_color(t.markdown_bold()),
        italic: rgba_from_color(t.markdown_italic()),
        strikethrough: rgba_from_color(t.markdown_strikethrough()),
        code_fg: rgba_from_color(t.markdown_code_foreground()),
        code_bg: rgba_from_color(t.markdown_code_background()),
        code_block_bg: rgba_from_color(t.markdown_code_block_background()),
        code_block_border: rgba_from_color(t.markdown_code_block_border()),
        blockquote_fg: rgba_from_color(t.markdown_blockquote_foreground()),
        blockquote_bar: rgba_from_color(t.markdown_blockquote_bar()),
        link: rgba_from_color(t.markdown_link()),
        footnote: rgba_from_color(t.markdown_footnote()),
        url: rgba_from_color(t.markdown_url()),
        image_alt: rgba_from_color(t.markdown_image_alt()),
        list_marker: rgba_from_color(t.markdown_list_marker()),
        checkbox_checked: rgba_from_color(t.markdown_checkbox_checked()),
        checkbox_unchecked: rgba_from_color(t.markdown_checkbox_unchecked()),
        hr: rgba_from_color(t.markdown_hr()),
        table_border: rgba_from_color(t.markdown_table_border()),
        table_header_bg: rgba_from_color(t.markdown_table_header_bg()),
        table_alignment_bg: rgba_from_color(t.markdown_table_alignment_bg()),
        table_active_cell_outline: rgba_from_color(t.markdown_table_active_cell_outline()),
        inline_highlight_fg: rgba_from_color(t.editor_inline_highlight_foreground()),
        inline_highlight_bg: rgba_from_color(t.editor_inline_highlight_background()),
        formula_value: rgba_from_color(t.markdown_formula_value()),
        formula_error: rgba_from_color(t.markdown_formula_error()),
    }
}

/// Sample the OS `system_dark` flag at startup. Defaults to `true`
/// (assume dark) when the registry / shell API is unreachable so we don't
/// flash the wrong palette on first paint.
#[must_use]
pub(crate) fn read_system_dark() -> bool {
    // Best effort: read `AppsUseLightTheme` from
    // HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize.
    // We avoid the full `winreg` crate (per spec §1 — every dependency does
    // one thing) and call `RegGetValueW` directly via `windows-sys`.
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let mut data: u32 = 1; // 1 = light is the registry's "1 = light"; 0 = dark.
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = w!("AppsUseLightTheme");
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::addr_of_mut!(data).cast()),
            Some(&mut size),
        )
    };
    if res.is_err() {
        // Couldn't read — pick dark as the more notes-friendly default.
        return true;
    }
    data == 0
}

/// `WM_SETTINGCHANGE` arrives with `lParam` pointing at a UTF-16 string.
/// Returns `true` when the message describes an immersive-color change
/// (Windows dark/light flip). Reads at most a small fixed prefix.
#[must_use]
pub(crate) fn lparam_is_immersive_color_set(lparam: LPARAM) -> bool {
    let ptr = lparam.0 as *const u16;
    if ptr.is_null() {
        return false;
    }
    // "ImmersiveColorSet" is 17 wide chars; read up to 32 plus terminator.
    let mut buf = [0u16; 32];
    for (i, slot) in buf.iter_mut().enumerate() {
        let ch = unsafe { *ptr.add(i) };
        if ch == 0 {
            break;
        }
        *slot = ch;
    }
    let s = String::from_utf16_lossy(&buf);
    let trimmed = s.trim_end_matches('\0');
    trimmed == "ImmersiveColorSet"
}

// Touch one symbol from `windows::Win32::UI::WindowsAndMessaging` to keep
// the import set consistent with future blink-on-pause work in Phase 11.
const _: fn() = || {
    let _: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS = SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);
    let _ = SystemParametersInfoW;
    let _ = SPI_GETCLIENTAREAANIMATION;
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_conversion_round_trips_sample() {
        let c = Color::rgba(0xff, 0x80, 0x00, 0x80);
        let r = rgba_from_color(c);
        assert!((r.r - 1.0).abs() < 1e-6);
        assert!((r.g - 128.0 / 255.0).abs() < 1e-6);
        assert!((r.b - 0.0).abs() < 1e-6);
        assert!((r.a - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn editor_colors_from_bundled_dark_populated() {
        let t = continuity_theme::assets::bundled_dark().unwrap();
        let c = editor_colors_from(&t);
        // Sanity: not the zero default for any field.
        assert!(c.bg.r > 0.0 || c.bg.g > 0.0 || c.bg.b > 0.0);
        assert!(c.line_highlight.a > 0.0);
        assert!(c.search_match_active.a > 0.0);
    }

    #[test]
    fn markdown_colors_from_bundled_dark_populated() {
        let t = continuity_theme::assets::bundled_dark().unwrap();
        let c = markdown_colors_from(&t);
        assert!(c.heading[0].r > 0.0); // h1 has color
        assert!(c.code_block_bg.r > 0.0);
        assert!(c.list_marker.r > 0.0);
        assert!(c.checkbox_checked.g > 0.0);
        assert!(c.table_border.r > 0.0);
    }

    #[test]
    fn cycle_mode_walks_dark_light_system() {
        let mut a = ActiveTheme::bundled().unwrap();
        a.set_mode(Mode::Dark);
        assert_eq!(a.cycle_mode(), Mode::Light);
        assert_eq!(a.cycle_mode(), Mode::System);
        assert_eq!(a.cycle_mode(), Mode::Dark);
    }

    #[test]
    fn system_dark_change_only_affects_system_mode() {
        let mut a = ActiveTheme::bundled().unwrap();
        a.set_mode(Mode::Dark);
        a.set_system_dark(false);
        // Dark mode pinned: still deep_minimal regardless of OS.
        assert_eq!(a.current.name, "deep_minimal");
        a.set_mode(Mode::System);
        a.set_system_dark(false);
        assert_eq!(a.current.name, "paper");
        a.set_system_dark(true);
        assert_eq!(a.current.name, "deep_minimal");
    }
}
