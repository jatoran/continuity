//! [`Theme`] struct, TOML loader, and typed-accessor surface.
//!
//! Two parse entry points:
//!
//! - [`Theme::from_toml`] returns the raw struct without validation. Useful
//!   for tests and for tooling that wants to inspect a partial theme.
//! - [`Theme::load`] parses then validates that every required key from
//!   [`crate::keys::REQUIRED_KEYS`] is present. After [`Theme::load`]
//!   succeeds, the typed accessors below cannot panic on missing keys.
//!
//! Thread ownership: a `Theme` is plain data and is `Clone`; the active
//! `Theme` for a window is owned by that window's UI thread.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{keys::REQUIRED_KEYS, Color, Error};

mod markdown_accessors;

/// A theme: name + flat color table keyed by dot-separated paths
/// (e.g., `editor.background`, `markdown.heading.1`).
#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    /// Display name.
    pub name: String,
    /// Color table. Keys must include every entry in
    /// [`crate::keys::REQUIRED_KEYS`] for [`Theme::load`] to accept it.
    #[serde(default)]
    pub colors: BTreeMap<String, Color>,
}

impl Theme {
    /// Parse a theme from a TOML string without validating required keys.
    ///
    /// Use [`Theme::load`] to parse and validate in one step.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the TOML is malformed and
    /// [`Error::InvalidColor`] if any color value can't be parsed.
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        toml::from_str(s).map_err(Error::Parse)
    }

    /// Parse a theme from a TOML string and validate that every required key
    /// from [`crate::keys::REQUIRED_KEYS`] is present.
    ///
    /// # Errors
    ///
    /// Returns the same parse errors as [`Theme::from_toml`], or
    /// [`Error::MissingKey`] when any required dot-key is absent.
    pub fn load(s: &str) -> Result<Self, Error> {
        let theme = Self::from_toml(s)?;
        theme.validate_required()?;
        Ok(theme)
    }

    /// Look up a color by dot-key.
    #[must_use]
    pub fn color(&self, key: &str) -> Option<Color> {
        self.colors.get(key).copied()
    }

    /// Verify every required key is present. The static `&'static str`
    /// stored in [`Error::MissingKey`] is the first absent key in
    /// [`crate::keys::REQUIRED_KEYS`] order so the error message is stable.
    ///
    /// # Errors
    ///
    /// [`Error::MissingKey`] naming the absent key.
    pub(crate) fn validate_required(&self) -> Result<(), Error> {
        for key in REQUIRED_KEYS {
            if !self.colors.contains_key(*key) {
                return Err(Error::MissingKey(key));
            }
        }
        Ok(())
    }

    /// Look up a color by key, returning the validated value. After
    /// [`Theme::load`] succeeds, every key listed in
    /// [`crate::keys::REQUIRED_KEYS`] is guaranteed to be present.
    #[must_use]
    pub(crate) fn required(&self, key: &'static str) -> Color {
        self.colors
            .get(key)
            .copied()
            .expect("invariant: required theme key missing after validate_required")
    }

    // --- typed accessors ------------------------------------------------

    /// `window.background`.
    #[must_use]
    pub fn window_background(&self) -> Color {
        self.required("window.background")
    }
    /// `window.foreground`.
    #[must_use]
    pub fn window_foreground(&self) -> Color {
        self.required("window.foreground")
    }
    /// `panel.background`.
    #[must_use]
    pub fn panel_background(&self) -> Color {
        self.required("panel.background")
    }
    /// `panel.foreground`.
    #[must_use]
    pub fn panel_foreground(&self) -> Color {
        self.required("panel.foreground")
    }
    /// `panel.active_tab.background`.
    #[must_use]
    pub fn panel_active_tab_background(&self) -> Color {
        self.required("panel.active_tab.background")
    }
    /// `panel.active_tab.foreground`.
    #[must_use]
    pub fn panel_active_tab_foreground(&self) -> Color {
        self.required("panel.active_tab.foreground")
    }
    /// `panel.inactive_tab.background`.
    #[must_use]
    pub fn panel_inactive_tab_background(&self) -> Color {
        self.required("panel.inactive_tab.background")
    }
    /// `panel.inactive_tab.foreground`.
    #[must_use]
    pub fn panel_inactive_tab_foreground(&self) -> Color {
        self.required("panel.inactive_tab.foreground")
    }
    /// `pane.border`.
    #[must_use]
    pub fn pane_border(&self) -> Color {
        self.required("pane.border")
    }
    /// `pane.border_active`.
    #[must_use]
    pub fn pane_border_active(&self) -> Color {
        self.required("pane.border_active")
    }

    /// `editor.background`.
    #[must_use]
    pub fn editor_background(&self) -> Color {
        self.required("editor.background")
    }
    /// `editor.foreground`.
    #[must_use]
    pub fn editor_foreground(&self) -> Color {
        self.required("editor.foreground")
    }
    /// §H1 `editor.foreground_dim` — RGB used to tint non-focused source
    /// ranges when the focus-mode dim pass is active. The alpha channel
    /// of this key is ignored; alpha comes from
    /// [`Self::editor_focus_dim_alpha`].
    #[must_use]
    pub fn editor_foreground_dim(&self) -> Color {
        self.required("editor.foreground_dim")
    }
    /// §H1 `editor.focus_dim_alpha` — encoded as `#RRGGBBAA`; **only the
    /// alpha channel is consumed** by the renderer. RGB is intentionally
    /// ignored (the visible color comes from
    /// [`Self::editor_foreground_dim`]). Returns the raw `Color` so the
    /// caller can pick `.a`.
    #[must_use]
    pub fn editor_focus_dim_alpha(&self) -> Color {
        self.required("editor.focus_dim_alpha")
    }
    /// `editor.cursor.primary`.
    #[must_use]
    pub fn editor_cursor_primary(&self) -> Color {
        self.required("editor.cursor.primary")
    }
    /// `editor.cursor.secondary`.
    #[must_use]
    pub fn editor_cursor_secondary(&self) -> Color {
        self.required("editor.cursor.secondary")
    }
    /// `editor.selection`.
    #[must_use]
    pub fn editor_selection(&self) -> Color {
        self.required("editor.selection")
    }
    /// `editor.selection_inactive`.
    #[must_use]
    pub fn editor_selection_inactive(&self) -> Color {
        self.required("editor.selection_inactive")
    }
    /// `editor.line_highlight`.
    #[must_use]
    pub fn editor_line_highlight(&self) -> Color {
        self.required("editor.line_highlight")
    }
    /// `editor.line_number`.
    #[must_use]
    pub fn editor_line_number(&self) -> Color {
        self.required("editor.line_number")
    }
    /// `editor.line_number_active`.
    #[must_use]
    pub fn editor_line_number_active(&self) -> Color {
        self.required("editor.line_number_active")
    }
    /// `editor.indent_guide`.
    #[must_use]
    pub fn editor_indent_guide(&self) -> Color {
        self.required("editor.indent_guide")
    }
    /// `editor.indent_guide_active`.
    #[must_use]
    pub fn editor_indent_guide_active(&self) -> Color {
        self.required("editor.indent_guide_active")
    }
    /// `editor.search_match`.
    #[must_use]
    pub fn editor_search_match(&self) -> Color {
        self.required("editor.search_match")
    }
    /// `editor.search_match_active`.
    #[must_use]
    pub fn editor_search_match_active(&self) -> Color {
        self.required("editor.search_match_active")
    }
    /// `editor.find_bar.background`.
    #[must_use]
    pub fn editor_find_bar_background(&self) -> Color {
        self.required("editor.find_bar.background")
    }

    /// `editor.search_minimap.background` — Phase G4 search-active
    /// minimap strip fill (right edge of pane while the find bar is open).
    #[must_use]
    pub fn editor_search_minimap_background(&self) -> Color {
        self.required("editor.search_minimap.background")
    }
    /// `editor.search_minimap.match` — per-match tick color.
    #[must_use]
    pub fn editor_search_minimap_match(&self) -> Color {
        self.required("editor.search_minimap.match")
    }
    /// `editor.search_minimap.match_active` — focused-match tick color.
    #[must_use]
    pub fn editor_search_minimap_match_active(&self) -> Color {
        self.required("editor.search_minimap.match_active")
    }

    /// `editor.minimap.background` — strip fill for the scaled-text
    /// minimap (right edge of pane while `[ui].show_minimap` is on).
    #[must_use]
    pub fn editor_minimap_background(&self) -> Color {
        self.required("editor.minimap.background")
    }
    /// `editor.minimap.foreground` — text color for the scaled-down
    /// glyphs inside the minimap strip. Typically a low-alpha tint of
    /// the editor foreground so the strip reads as a thumbnail.
    #[must_use]
    pub fn editor_minimap_foreground(&self) -> Color {
        self.required("editor.minimap.foreground")
    }
    /// `editor.minimap.viewport_indicator` — the translucent box drawn
    /// over the section of the minimap currently visible in the editor.
    /// Should be partially transparent so the underlying glyphs read
    /// through.
    #[must_use]
    pub fn editor_minimap_viewport_indicator(&self) -> Color {
        self.required("editor.minimap.viewport_indicator")
    }

    /// `editor.loading_overlay.background` — P0.8.3 translucent fill
    /// for the transient "building view" overlay drawn while paint waits
    /// on a slow projection-worker build.
    #[must_use]
    pub fn editor_loading_overlay_background(&self) -> Color {
        self.required("editor.loading_overlay.background")
    }

    /// `editor.loading_overlay.foreground` — P0.8.3 label text color.
    #[must_use]
    pub fn editor_loading_overlay_foreground(&self) -> Color {
        self.required("editor.loading_overlay.foreground")
    }

    /// `editor.loading_overlay.border` — P0.8.3 1-DIP panel stroke.
    /// Alpha `0` skips the stroke.
    #[must_use]
    pub fn editor_loading_overlay_border(&self) -> Color {
        self.required("editor.loading_overlay.border")
    }

    /// `editor.caret_jump_glow` — Phase B6 RGBA tint applied to the
    /// destination row of a long caret jump.
    #[must_use]
    pub fn editor_caret_jump_glow(&self) -> Color {
        self.required("editor.caret_jump_glow")
    }

    /// `editor.edit_pulse` — α.1 RGBA tint painted over the affected
    /// source rows of a structural edit (paste, duplicate, move-line,
    /// undo target, smart-expand boundary).
    #[must_use]
    pub fn editor_edit_pulse(&self) -> Color {
        self.required("editor.edit_pulse")
    }

    /// `editor.soft_wrap_indicator` — Phase B17 margin glyph color.
    #[must_use]
    pub fn editor_soft_wrap_indicator(&self) -> Color {
        self.required("editor.soft_wrap_indicator")
    }

    /// `editor.breadcrumb.foreground` — Phase F1 default text color
    /// for non-active breadcrumb segments.
    #[must_use]
    pub fn editor_breadcrumb_foreground(&self) -> Color {
        self.required("editor.breadcrumb.foreground")
    }

    /// `editor.breadcrumb.separator` — Phase F1 `›` separator color.
    #[must_use]
    pub fn editor_breadcrumb_separator(&self) -> Color {
        self.required("editor.breadcrumb.separator")
    }

    /// `editor.breadcrumb.active` — Phase F1 color for the innermost
    /// (current) heading segment.
    #[must_use]
    pub fn editor_breadcrumb_active(&self) -> Color {
        self.required("editor.breadcrumb.active")
    }

    /// `editor.outline.background` — Phase F2 outline-sidebar fill.
    #[must_use]
    pub fn editor_outline_background(&self) -> Color {
        self.required("editor.outline.background")
    }

    /// `editor.outline.foreground` — Phase F2 default row text.
    #[must_use]
    pub fn editor_outline_foreground(&self) -> Color {
        self.required("editor.outline.foreground")
    }

    /// `editor.outline.foreground_active` — Phase F2 row text for the
    /// heading containing the caret.
    #[must_use]
    pub fn editor_outline_foreground_active(&self) -> Color {
        self.required("editor.outline.foreground_active")
    }

    /// `editor.outline.separator` — Phase F2 vertical rule between the
    /// sidebar and the editor body.
    #[must_use]
    pub fn editor_outline_separator(&self) -> Color {
        self.required("editor.outline.separator")
    }

    /// `editor.inline_highlight.foreground` — Phase F3 text color
    /// inside an `==…==` highlight span. Falls back to
    /// `editor.foreground` when themes leave it equal to the default.
    #[must_use]
    pub fn editor_inline_highlight_foreground(&self) -> Color {
        self.required("editor.inline_highlight.foreground")
    }

    /// `editor.inline_highlight.background` — Phase F3 background fill
    /// painted behind an `==…==` highlight span (`yellow-by-theme`).
    #[must_use]
    pub fn editor_inline_highlight_background(&self) -> Color {
        self.required("editor.inline_highlight.background")
    }

    /// `editor.pair_rainbow.N` — Phase B8 nested-bracket palette
    /// indexed by depth `%` 6. Out-of-range `level` clamps to the
    /// nearest valid slot.
    #[must_use]
    pub fn editor_pair_rainbow(&self, level: u8) -> Color {
        let key = match level.clamp(0, 5) {
            0 => "editor.pair_rainbow.0",
            1 => "editor.pair_rainbow.1",
            2 => "editor.pair_rainbow.2",
            3 => "editor.pair_rainbow.3",
            4 => "editor.pair_rainbow.4",
            _ => "editor.pair_rainbow.5",
        };
        self.required(key)
    }

    /// `status.background`.
    #[must_use]
    pub fn status_background(&self) -> Color {
        self.required("status.background")
    }
    /// `status.foreground`.
    #[must_use]
    pub fn status_foreground(&self) -> Color {
        self.required("status.foreground")
    }
    /// `status.error`.
    #[must_use]
    pub fn status_error(&self) -> Color {
        self.required("status.error")
    }
    /// `status.warn`.
    #[must_use]
    pub fn status_warn(&self) -> Color {
        self.required("status.warn")
    }
    /// `status.info`.
    #[must_use]
    pub fn status_info(&self) -> Color {
        self.required("status.info")
    }

    /// `overlay.background`.
    #[must_use]
    pub fn overlay_background(&self) -> Color {
        self.required("overlay.background")
    }
    /// `overlay.shadow`.
    #[must_use]
    pub fn overlay_shadow(&self) -> Color {
        self.required("overlay.shadow")
    }
    /// `palette.background`.
    #[must_use]
    pub fn palette_background(&self) -> Color {
        self.required("palette.background")
    }
    /// `palette.match_highlight`.
    #[must_use]
    pub fn palette_match_highlight(&self) -> Color {
        self.required("palette.match_highlight")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
name = "deep_minimal"

[colors]
"editor.background"    = "#1a1a1a"
"editor.foreground"    = "#e0e0e0"
"editor.cursor.primary" = "#ff8800"
"##;

    #[test]
    fn parses_sample_theme() {
        let t = Theme::from_toml(SAMPLE).unwrap();
        assert_eq!(t.name, "deep_minimal");
        assert_eq!(
            t.color("editor.background"),
            Some(Color::rgb(0x1a, 0x1a, 0x1a))
        );
        assert_eq!(
            t.color("editor.cursor.primary"),
            Some(Color::rgb(0xff, 0x88, 0x00))
        );
        assert_eq!(t.color("nonexistent"), None);
    }

    #[test]
    fn rejects_invalid_color() {
        let bad = r##"
name = "x"
[colors]
"editor.background" = "not-a-color"
"##;
        assert!(Theme::from_toml(bad).is_err());
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(Theme::from_toml("name =").is_err());
    }

    #[test]
    fn empty_colors_section_ok() {
        let t = Theme::from_toml(r#"name = "empty""#).unwrap();
        assert_eq!(t.name, "empty");
        assert!(t.colors.is_empty());
    }

    #[test]
    fn validate_required_rejects_partial_theme() {
        let t = Theme::from_toml(SAMPLE).unwrap();
        let err = t.validate_required().unwrap_err();
        // First missing key from REQUIRED_KEYS order — i.e. window.background.
        assert!(matches!(err, Error::MissingKey("window.background")));
    }

    #[test]
    fn load_rejects_partial_theme() {
        let err = Theme::load(SAMPLE).unwrap_err();
        assert!(matches!(err, Error::MissingKey(_)));
    }
}
