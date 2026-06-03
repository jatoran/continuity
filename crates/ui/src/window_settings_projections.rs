//! δ.6 Tier 2 — sub-state + pure helpers used by
//! [`crate::Window::apply_settings`] to project a validated
//! [`continuity_config::Settings`] onto runtime state.
//!
//! Why a sub-state: the 600-line cap on `crates/ui/src/window.rs` is
//! unconditional. Bundling the new projections onto a
//! `SettingsProjections` field keeps the canonical `Window` struct under
//! the cap and keeps the new fields adjacent to their projection logic.
//!
//! Why pure helpers: the projection target is `&mut Window`, and a
//! `Window` requires a real HWND, so we cannot construct one in a unit
//! test without spawning a desktop window through the harness. Each
//! transformation that needs validation (clamping, glyph parsing,
//! length-checking) lands as a free function exercised by the test
//! module at the bottom of this file.
//!
//! Each projection mirrors a single hot-reload contract (A) entry from
//! `.docs/design/defaults.md` "Hot-reload contract": TOML wins on
//! reload; the runtime field is the canonical snapshot.
//!
//! Thread ownership: `SettingsProjections` is owned by the window's UI
//! thread. The pure helpers are called only from the UI thread through
//! `apply_settings`.

use continuity_config::{EditorConfig, MarkdownConfig, MarkdownDialect, RevealMode, Settings};
use continuity_display_map::MarkdownRenderToggles;

/// Default per-level heading scale. Matches
/// [`continuity_config::MarkdownConfig::default`]. Used when the
/// settings vec is the wrong length (`validate()` rejects that case
/// before the projection runs, but we belt-and-brace defensively so a
/// malformed snapshot never panics the UI thread).
pub(crate) const DEFAULT_HEADING_SCALE: [f32; 6] = [2.0, 1.6, 1.35, 1.2, 1.1, 1.05];

/// Default soft-wrap indicator glyph. Matches
/// [`continuity_config::EditorConfig::default`]; the literal is `↪`
/// (LEFTWARDS ARROW WITH HOOK, U+21AA).
pub(crate) const DEFAULT_SOFT_WRAP_GLYPH: char = '\u{21AA}';

/// δ.6 Tier 2 sub-state mirrored from `Settings` by
/// [`crate::Window::apply_settings`]. Every field is contract (A): TOML
/// wins on reload; the runtime field is the canonical snapshot.
///
/// Downstream consumers (decoration / autocorrect / render) read these
/// fields per-paint or per-keystroke so the next frame after a reload
/// sees the new value without further wiring.
#[derive(Debug, Clone)]
pub(crate) struct SettingsProjections {
    /// Mirrors `[editor].font_family_mono`. Consumed by future
    /// monospace-font code-block paths; today it is the canonical
    /// landing site so a `settings.toml` edit propagates to runtime
    /// without a restart.
    pub(crate) mono_font_family: String,
    /// Mirrors `[editor].line_height`. Consumed by render via
    /// `LINE_HEIGHT_DIP` derivation; cached here so a hot-reload picks
    /// the new multiplier on the next paint.
    pub(crate) line_height_multiplier: f32,
    /// Mirrors `[editor].zoom_step_pct`. Read by the Ctrl-scroll zoom
    /// handler so TOML edits affect runtime zoom step.
    pub(crate) zoom_step_pct: u32,
    /// Mirrors `[editor].show_soft_wrap_indicator`.
    pub(crate) show_soft_wrap_indicator: bool,
    /// Mirrors `[editor].soft_wrap_indicator_glyph` after validation
    /// (single-character).
    pub(crate) soft_wrap_indicator_glyph: char,
    /// Mirrors `[editor].smart_typography_enabled`.
    pub(crate) smart_typography_enabled: bool,
    /// Mirrors `[editor].autolink_bare_urls`.
    pub(crate) autolink_bare_urls: bool,
    /// Mirrors `[editor].autocorrect_enabled`.
    pub(crate) autocorrect_enabled: bool,
    /// Mirrors `[markdown].reveal_mode`.
    pub(crate) markdown_reveal_mode: RevealMode,
    /// Mirrors `[markdown].heading_scale` (always 6 entries per
    /// validation).
    pub(crate) markdown_heading_scale: [f32; 6],
    /// Mirrors `[markdown].dialect`.
    pub(crate) markdown_dialect: MarkdownDialect,
    /// Mirrors the five `[markdown].render_*` decoration toggles
    /// (`render_italic` / `render_bold` / `render_highlight` /
    /// `render_setext_heading` / `render_divider`). Threaded into the
    /// display-map builder and render paint gates per frame so a
    /// hot-reload flip takes effect on the next paint. The toggle set is
    /// also folded into the font-state key (see
    /// [`crate::Window::current_font_state_id`]) so cached frames /
    /// segment lists / wrap profiles built against the previous toggles
    /// are invalidated.
    pub(crate) markdown_render_toggles: MarkdownRenderToggles,
    /// δ.6 Tier 3 — suppression counter for `settings.toml` writebacks.
    /// Incremented by [`crate::window_settings_persist`] when a toggle
    /// command persists a boolean; the next inbound
    /// [`continuity_config::ConfigEvent::Settings`] decrements it and
    /// skips re-applying so our own writeback does not echo through
    /// `apply_settings`. Counter rather than bool to tolerate a burst
    /// of rapid toggles within a single debounce window.
    pub(crate) writeback_in_flight: u32,
}

impl Default for SettingsProjections {
    fn default() -> Self {
        let editor = EditorConfig::default();
        Self {
            mono_font_family: editor.font_family_mono,
            line_height_multiplier: editor.line_height,
            zoom_step_pct: editor.zoom_step_pct,
            show_soft_wrap_indicator: editor.show_soft_wrap_indicator,
            soft_wrap_indicator_glyph: soft_wrap_glyph_from_setting(
                &editor.soft_wrap_indicator_glyph,
            ),
            smart_typography_enabled: editor.smart_typography_enabled,
            autolink_bare_urls: editor.autolink_bare_urls,
            autocorrect_enabled: editor.autocorrect_enabled,
            markdown_reveal_mode: RevealMode::Block,
            markdown_heading_scale: DEFAULT_HEADING_SCALE,
            markdown_dialect: MarkdownDialect::Gfm,
            markdown_render_toggles: markdown_render_toggles_from_config(&MarkdownConfig::default()),
            writeback_in_flight: 0,
        }
    }
}

impl SettingsProjections {
    /// Apply the contract-(A) projections that do not need to invalidate
    /// the font-state cache (those still live on
    /// [`crate::Window::apply_settings`] so they can wrap themselves in
    /// `with_caret_line_anchored`). This method is the canonical
    /// landing site for every other Tier-2 projection.
    pub(crate) fn apply_from_settings(&mut self, s: &Settings) {
        self.zoom_step_pct = s.editor.zoom_step_pct;
        self.show_soft_wrap_indicator = s.editor.show_soft_wrap_indicator;
        self.soft_wrap_indicator_glyph =
            soft_wrap_glyph_from_setting(&s.editor.soft_wrap_indicator_glyph);
        self.smart_typography_enabled = s.editor.smart_typography_enabled;
        self.autolink_bare_urls = s.editor.autolink_bare_urls;
        self.autocorrect_enabled = s.editor.autocorrect_enabled;
        self.markdown_reveal_mode = s.reveal_mode();
        self.markdown_dialect = s.markdown_dialect();
        self.markdown_heading_scale = heading_scale_from_slice(&s.markdown.heading_scale);
        self.markdown_render_toggles = markdown_render_toggles_from_config(&s.markdown);
    }
}

/// Project the five `[markdown].render_*` booleans into the display-map
/// [`MarkdownRenderToggles`] value threaded through the builder + render
/// paint gates.
#[must_use]
pub(crate) fn markdown_render_toggles_from_config(
    markdown: &MarkdownConfig,
) -> MarkdownRenderToggles {
    MarkdownRenderToggles {
        italic: markdown.render_italic,
        bold: markdown.render_bold,
        highlight: markdown.render_highlight,
        setext_heading: markdown.render_setext_heading,
        divider: markdown.render_divider,
    }
}

/// Convert a validated `[markdown].heading_scale` slice into a
/// fixed-size `[f32; 6]`. `validate()` guarantees length == 6 and per-
/// element range, so the happy path is a copy. A wrong-length input
/// falls back to the bundled default rather than panic — defensive
/// only; no production caller should ever hit the fallback.
#[must_use]
pub(crate) fn heading_scale_from_slice(scales: &[f32]) -> [f32; 6] {
    if scales.len() == 6 {
        let mut out = [0.0_f32; 6];
        out.copy_from_slice(scales);
        out
    } else {
        DEFAULT_HEADING_SCALE
    }
}

/// Resolve `[editor].soft_wrap_indicator_glyph` (a `String`) to the
/// single `char` the renderer needs. `validate()` rejects non-single-
/// character strings, so the happy path always finds the first char.
/// An empty string (validation pre-empted, but defensive) falls back
/// to the bundled default.
#[must_use]
pub(crate) fn soft_wrap_glyph_from_setting(glyph: &str) -> char {
    glyph.chars().next().unwrap_or(DEFAULT_SOFT_WRAP_GLYPH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_scale_copies_six_element_slice_verbatim() {
        let input = [3.0, 2.0, 1.5, 1.25, 1.1, 1.0];
        let out = heading_scale_from_slice(&input);
        assert_eq!(out, input);
    }

    #[test]
    fn heading_scale_falls_back_when_length_wrong() {
        // validate() rejects this; the helper still returns sensible
        // defaults rather than panicking.
        let too_short = [1.0_f32, 1.0, 1.0];
        assert_eq!(heading_scale_from_slice(&too_short), DEFAULT_HEADING_SCALE);
        let too_long = [1.0_f32; 8];
        assert_eq!(heading_scale_from_slice(&too_long), DEFAULT_HEADING_SCALE);
    }

    #[test]
    fn soft_wrap_glyph_picks_first_char() {
        assert_eq!(soft_wrap_glyph_from_setting("↪"), '\u{21AA}');
        assert_eq!(soft_wrap_glyph_from_setting("→"), '\u{2192}');
    }

    #[test]
    fn soft_wrap_glyph_empty_falls_back_to_default() {
        // validate() rejects this; the helper still returns the
        // bundled default rather than producing an unrenderable
        // sentinel.
        assert_eq!(soft_wrap_glyph_from_setting(""), DEFAULT_SOFT_WRAP_GLYPH);
    }

    fn settings_with(toml: &str) -> Settings {
        Settings::from_toml_validated(toml).expect("validated TOML")
    }

    #[test]
    fn defaults_match_editor_and_markdown_defaults() {
        let p = SettingsProjections::default();
        let editor = EditorConfig::default();
        assert_eq!(p.mono_font_family, editor.font_family_mono);
        assert!((p.line_height_multiplier - editor.line_height).abs() < f32::EPSILON);
        assert_eq!(p.zoom_step_pct, editor.zoom_step_pct);
        assert_eq!(p.show_soft_wrap_indicator, editor.show_soft_wrap_indicator);
        assert_eq!(p.soft_wrap_indicator_glyph, '\u{21AA}');
        assert_eq!(p.smart_typography_enabled, editor.smart_typography_enabled);
        assert_eq!(p.autolink_bare_urls, editor.autolink_bare_urls);
        assert_eq!(p.autocorrect_enabled, editor.autocorrect_enabled);
        assert_eq!(p.markdown_reveal_mode, RevealMode::Block);
        assert_eq!(p.markdown_dialect, MarkdownDialect::Gfm);
        assert_eq!(p.markdown_heading_scale, DEFAULT_HEADING_SCALE);
    }

    #[test]
    fn apply_from_settings_projects_reveal_mode() {
        let s = settings_with("[markdown]\nreveal_mode = \"line\"\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert_eq!(p.markdown_reveal_mode, RevealMode::Line);
    }

    #[test]
    fn apply_from_settings_projects_markdown_dialect() {
        let s = settings_with("[markdown]\ndialect = \"commonmark\"\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert_eq!(p.markdown_dialect, MarkdownDialect::CommonMark);
    }

    #[test]
    fn apply_from_settings_projects_heading_scale() {
        let s = settings_with("[markdown]\nheading_scale = [3.0, 2.0, 1.5, 1.25, 1.1, 1.0]\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert_eq!(p.markdown_heading_scale, [3.0, 2.0, 1.5, 1.25, 1.1, 1.0]);
    }

    #[test]
    fn apply_from_settings_projects_smart_typography_off() {
        let s = settings_with("[editor]\nsmart_typography_enabled = false\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert!(!p.smart_typography_enabled);
    }

    #[test]
    fn apply_from_settings_projects_autolink_bare_urls_off() {
        let s = settings_with("[editor]\nautolink_bare_urls = false\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert!(!p.autolink_bare_urls);
    }

    #[test]
    fn apply_from_settings_projects_autocorrect_enabled_on() {
        let s = settings_with("[editor]\nautocorrect_enabled = true\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert!(p.autocorrect_enabled);
    }

    #[test]
    fn apply_from_settings_projects_show_soft_wrap_indicator_off() {
        let s = settings_with("[editor]\nshow_soft_wrap_indicator = false\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert!(!p.show_soft_wrap_indicator);
    }

    #[test]
    fn apply_from_settings_projects_soft_wrap_glyph() {
        // validate() requires single-character glyph.
        let s = settings_with("[editor]\nsoft_wrap_indicator_glyph = \"→\"\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert_eq!(p.soft_wrap_indicator_glyph, '\u{2192}');
    }

    #[test]
    fn apply_from_settings_projects_zoom_step_pct() {
        let s = settings_with("[editor]\nzoom_step_pct = 25\n");
        let mut p = SettingsProjections::default();
        p.apply_from_settings(&s);
        assert_eq!(p.zoom_step_pct, 25);
    }
}
