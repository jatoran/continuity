//! `Settings`: a deserialized `settings.toml`.
//!
//! All sections are optional (`#[serde(default)]`) so a missing or partial
//! config still loads. Defaults match the spec's recommended values.

use serde::Deserialize;

use crate::focus::FocusConfig;
use crate::mode::{
    CaretStyle, IndentType, MarkdownDialect, PersistenceMode, RevealMode, StatusBarSegment,
    TabCloseButton, ThemeMode,
};
use crate::settings_backup::BackupConfig;
use crate::settings_markdown::MarkdownConfig;
use crate::settings_window::WindowConfig;
use crate::workers::WorkerConfig;
use crate::Error;

/// The full settings file.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Persistence behavior (debounce, snapshot cadence, trash retention).
    pub persistence: PersistenceConfig,
    /// Hot-backup cadence and retention.
    pub backup: BackupConfig,
    /// Editor visual + behavioral preferences.
    pub editor: EditorConfig,
    /// Markdown-specific preferences.
    pub markdown: MarkdownConfig,
    /// UI-chrome preferences.
    pub ui: UiConfig,
    /// Status-bar segment list + format toggles.
    pub statusbar: StatusBarConfig,
    /// Window-restoration preferences.
    pub window: WindowConfig,
    /// Phase G2 — find-bar behavior.
    pub find: FindConfig,
    /// Phase H1/H2 — focus modes (granular + distraction-free).
    pub focus: FocusConfig,
    /// Worker-thread watchdogs.
    pub workers: WorkerConfig,
}

impl Settings {
    /// Parse a `Settings` from TOML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] for malformed TOML.
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        toml::from_str(s).map_err(Error::Parse)
    }

    /// Parse + validate. Use this on every load — bare [`Self::from_toml`]
    /// is preserved for tests that want to inspect malformed-but-parseable
    /// values.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] for malformed TOML or [`Error::Invalid`]
    /// for an out-of-range / unknown enum value.
    pub fn from_toml_validated(s: &str) -> Result<Self, Error> {
        let parsed = Self::from_toml(s)?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Typed view of `[persistence].mode`. Validation guarantees the
    /// underlying string parses; `expect` here is an invariant assertion.
    #[must_use]
    pub fn persistence_mode(&self) -> PersistenceMode {
        PersistenceMode::parse(&self.persistence.mode)
            .expect("invariant: validate() ensures persistence.mode is a known value")
    }

    /// Typed view of `[markdown].reveal_mode`.
    #[must_use]
    pub fn reveal_mode(&self) -> RevealMode {
        RevealMode::parse(&self.markdown.reveal_mode)
            .expect("invariant: validate() ensures markdown.reveal_mode is a known value")
    }

    /// Typed view of `[markdown].dialect` — Phase F7.
    #[must_use]
    pub fn markdown_dialect(&self) -> MarkdownDialect {
        MarkdownDialect::parse(&self.markdown.dialect)
            .expect("invariant: validate() ensures markdown.dialect is a known value")
    }

    /// Typed view of `[editor].caret_style`.
    #[must_use]
    pub fn caret_style(&self) -> CaretStyle {
        CaretStyle::parse(&self.editor.caret_style)
            .expect("invariant: validate() ensures editor.caret_style is a known value")
    }

    /// Typed view of `[editor].indent_type`.
    #[must_use]
    pub fn indent_type(&self) -> IndentType {
        IndentType::parse(&self.editor.indent_type)
            .expect("invariant: validate() ensures editor.indent_type is a known value")
    }

    /// Typed view of `[ui].theme`.
    #[must_use]
    pub fn theme_mode(&self) -> ThemeMode {
        ThemeMode::parse(&self.ui.theme)
            .expect("invariant: validate() ensures ui.theme is a known value")
    }

    /// Typed view of `[ui].tab_close_button`.
    #[must_use]
    pub fn tab_close_button(&self) -> TabCloseButton {
        TabCloseButton::parse(&self.ui.tab_close_button)
            .expect("invariant: validate() ensures ui.tab_close_button is a known value")
    }

    // `effective_autocorrect_rules` lives in `crate::effective_autocorrect`.

    /// Typed view of `[statusbar].segments`. Order follows the TOML list.
    #[must_use]
    pub fn status_bar_segments(&self) -> Vec<StatusBarSegment> {
        self.statusbar
            .segments
            .iter()
            .map(|s| {
                StatusBarSegment::parse(s)
                    .expect("invariant: validate() ensures statusbar.segments are known values")
            })
            .collect()
    }
}
/// `[persistence]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PersistenceConfig {
    /// `"balanced" | "max_safety" | "max_speed"`.
    pub mode: String,
    /// Edit-flush debounce in milliseconds.
    pub debounce_ms: u32,
    /// Take a snapshot every N edits.
    pub snapshot_every_edits: u32,
    /// Take a snapshot every N changed bytes.
    pub snapshot_every_bytes: u32,
    /// Trash retention in days.
    pub trash_retention_days: u32,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            mode: "balanced".into(),
            debounce_ms: 300,
            snapshot_every_edits: 500,
            snapshot_every_bytes: 262_144,
            trash_retention_days: 30,
        }
    }
}

/// `[editor]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Prose font family.
    pub font_family_prose: String,
    /// Monospace font family.
    pub font_family_mono: String,
    /// Base font size in points.
    pub font_size: f32,
    /// Global text-scale (zoom) multiplier applied on top of
    /// [`Self::font_size`] in every window. This is the single durable
    /// home for editor zoom: the `view.zoom_in`/`zoom_out`/`zoom_reset`
    /// commands and Ctrl+wheel write it back (contract C), so the zoom
    /// level survives relaunch and applies across every open window via
    /// the settings fan-out. Validated to
    /// [`crate::zoom::MIN_ZOOM`]`..=`[`crate::zoom::MAX_ZOOM`] — the same
    /// range the runtime view-state clamp uses. Default `1.0`.
    pub text_scale: f32,
    /// Line height multiplier.
    pub line_height: f32,
    /// Word wrap on/off.
    pub word_wrap: bool,
    /// Ruler column positions (e.g., `[80, 120]`).
    pub ruler_columns: Vec<u32>,
    /// `"bar" | "block" | "underline"`.
    pub caret_style: String,
    /// `"spaces" | "tabs"` — what `editor.indent` / `editor.outdent`
    /// insert and remove per level. `"spaces"` emits
    /// [`Self::indent_width`] spaces; `"tabs"` emits one tab character.
    /// Default `"tabs"`. Switching at runtime does not retroactively
    /// convert existing indentation — use `editor.spaces_to_tabs` /
    /// `editor.tabs_to_spaces` for that.
    pub indent_type: String,
    /// Spaces emitted per indent level when `indent_type = "spaces"`.
    /// Also drives the indent-guide column spacing. Validated `1..=16`.
    /// Default `4`.
    pub indent_width: u32,
    /// On-screen width of a literal tab character, in columns. Drives
    /// the rendered tab-stop (via DirectWrite incremental tab stops),
    /// the indent-guide geometry for tab-indented lines, and the
    /// spaces↔tabs conversion commands. Validated `1..=16`. Default `4`.
    pub tab_width: u32,
    /// Caret blink interval in ms. `0` disables blinking.
    pub caret_blink_ms: u32,
    /// Phase B4: bar-mode caret width in DIPs. Ignored for block /
    /// underline styles.
    pub caret_width_px: u32,
    /// Phase B5: keep caret solid while actively typing; blink resumes
    /// after `caret_typing_pause_ms` idle.
    pub caret_blink_on_typing_pause: bool,
    /// Phase B5: idle threshold (ms) before blinking resumes when
    /// `caret_blink_on_typing_pause` is on.
    pub caret_typing_pause_ms: u32,
    /// α.3 — long-idle threshold (ms). `0` disables. Default `6000`.
    pub caret_long_idle_ms: u32,
    /// Phase B4: primary caret color. Either a `#rrggbb` / `#rrggbbaa`
    /// literal or a theme-key reference (e.g. `editor.cursor.primary`).
    /// Empty string falls through to the theme.
    pub caret_color: String,
    /// Phase B4: multi-cursor secondary color. Same syntax as
    /// `caret_color`; empty string falls through to the theme.
    pub caret_secondary_color: String,
    /// Phase B12: render bare URLs (`https://…`, `www.…`, emails)
    /// as clickable links via decoration. Default on.
    pub autolink_bare_urls: bool,
    /// Phase B14: strip trailing whitespace from every line as part
    /// of the save handler. One undo group. Default on.
    pub trim_trailing_whitespace_on_save: bool,
    /// When a file-associated buffer has no unexported edits and the
    /// file changes on disk (external tool, sync, reopen, or restore),
    /// silently reload the new bytes instead of prompting. A buffer
    /// with unexported edits always raises the reload / keep-mine /
    /// diff banner regardless of this toggle. Default on (auto-revert
    /// of unmodified buffers, matching common editor behavior).
    pub auto_revert_unmodified: bool,
    /// Phase B17: render a small `↪` glyph in the margin at every
    /// soft-wrap continuation row. Default on (effective only when
    /// `word_wrap = true`).
    pub show_soft_wrap_indicator: bool,
    /// Phase B17: Unicode glyph used for the soft-wrap indicator.
    /// Single character. Default `↪`.
    pub soft_wrap_indicator_glyph: String,
    /// Phase B18: enable the user-editable autocorrect rule pass.
    /// Default off so rule misfires don't surprise new users; the
    /// `%APPDATA%\continuity\autocorrect.toml` file ships empty.
    pub autocorrect_enabled: bool,
    /// γ — built-in smart-typography preset (curly quotes, en/em
    /// dash, ellipsis). Default `true`; independent of
    /// `autocorrect_enabled`.
    pub smart_typography_enabled: bool,
    /// Phase B7 caret-motion tween enable.
    pub caret_tween_enabled: bool,
    /// Phase B7: minimum display-row jump (`> N`) for tween to fire.
    pub caret_tween_threshold_rows: u32,
    /// Phase B7: tween duration in milliseconds.
    pub caret_tween_duration_ms: u32,
    /// Smooth-scrolling enabled.
    pub smooth_scroll: bool,
    /// Allow scrolling below the last line until it can sit at the
    /// viewport top (VS Code-style overscroll). A wheel/keyboard-only
    /// affordance — the scrollbar still pins to the true content bottom
    /// and Ctrl+End still lands the last line at the viewport bottom.
    /// Default `true`.
    pub scroll_past_end: bool,
    /// Mouse-wheel scroll speed multiplier. Default `2.0` doubles the
    /// base line-step distance.
    pub mouse_wheel_scroll_speed: f32,
    /// Ctrl-scroll zoom step in percent.
    pub zoom_step_pct: u32,
    /// Font ligatures enabled.
    pub ligatures: bool,
    /// Phase-16.5 auto-pair toggles. Each one enables auto-insertion
    /// of the matching close character when the named open is typed
    /// at a caret. Defaults follow spec §12: brackets and quotes on,
    /// emphasis chars (`*`, `_`) off — they hurt more than they help
    /// in prose.
    pub auto_pair_paren: bool,
    /// `[` → `[]`.
    pub auto_pair_bracket: bool,
    /// `{` → `{}`.
    pub auto_pair_brace: bool,
    /// `"` → `""`.
    pub auto_pair_dquote: bool,
    /// `'` → `''`.
    pub auto_pair_squote: bool,
    /// `` ` `` → `` `` ``.
    pub auto_pair_backtick: bool,
    /// `*` → `**`. Off by default.
    pub auto_pair_asterisk: bool,
    /// `_` → `__`. Off by default.
    pub auto_pair_underscore: bool,
    /// Phase H5 — enable the typed-`/` slash-command palette
    /// trigger. When `true`, typing `/` as the first non-whitespace
    /// character of a line opens the transient slash-command palette
    /// docked at the caret. Default `true`.
    pub slash_commands_enabled: bool,
    /// Phase H5 — user-overridable safelist for the slash-command
    /// palette. `None` (the default) populates the palette from every
    /// command flagged `palette_safe` in the command registry;
    /// `Some(list)` restricts the palette to exactly those command
    /// ids, in the order given. Unknown ids are silently skipped at
    /// build time so a stale `settings.toml` does not break the
    /// palette outright.
    pub slash_commands_palette: Option<Vec<String>>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            // §E9 — default prose font is the Windows-native variable
            // font, replacing the prior `Inter` default (§L#8). The
            // mono fallback chain `Cascadia Mono → Consolas` is handled
            // by DirectWrite's family-name fallback at draw time.
            font_family_prose: "Segoe UI Variable".into(),
            font_family_mono: "Cascadia Mono".into(),
            font_size: 14.0,
            text_scale: 1.0,
            line_height: 1.35,
            word_wrap: true,
            ruler_columns: Vec::new(),
            caret_style: "bar".into(),
            indent_type: "tabs".into(),
            indent_width: 4,
            tab_width: 4,
            caret_blink_ms: 530,
            caret_width_px: 2,
            caret_blink_on_typing_pause: true,
            caret_typing_pause_ms: 400,
            caret_long_idle_ms: 6_000,
            caret_color: String::new(),
            caret_secondary_color: String::new(),
            caret_tween_enabled: true,
            caret_tween_threshold_rows: 5,
            caret_tween_duration_ms: 160,
            autolink_bare_urls: true,
            trim_trailing_whitespace_on_save: true,
            auto_revert_unmodified: true,
            show_soft_wrap_indicator: true,
            soft_wrap_indicator_glyph: "↪".into(),
            autocorrect_enabled: false,
            smart_typography_enabled: true,
            smooth_scroll: true,
            scroll_past_end: true,
            mouse_wheel_scroll_speed: 2.0,
            zoom_step_pct: 10,
            ligatures: false,
            // Phase B8: auto-pair defaults flipped off across the board
            // (top user annoyance per defaults policy §J7).
            auto_pair_paren: false,
            auto_pair_bracket: false,
            auto_pair_brace: false,
            auto_pair_dquote: false,
            auto_pair_squote: false,
            auto_pair_backtick: false,
            auto_pair_asterisk: false,
            auto_pair_underscore: false,
            slash_commands_enabled: true,
            slash_commands_palette: None,
        }
    }
}

/// `[find]` section — Phase G2 find-bar behavior.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FindConfig {
    /// Restore the find bar's last query / replace text / mode flags
    /// when the user re-opens the bar in the same buffer. State is
    /// in-memory only — not persisted across sessions. Cleared when
    /// the buffer is closed. Default `true`.
    pub persist_per_buffer: bool,
}

impl Default for FindConfig {
    fn default() -> Self {
        Self {
            persist_per_buffer: true,
        }
    }
}

/// `[ui]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// `"system" | "dark" | "light"`.
    pub theme: String,
    /// Theme to use in dark mode.
    pub theme_dark: String,
    /// Theme to use in light mode.
    pub theme_light: String,
    /// Show line numbers.
    pub show_line_numbers: bool,
    /// Show relative line numbers for non-caret gutter rows.
    pub relative_line_numbers: bool,
    /// Show every visible gutter line number instead of only the caret line.
    pub show_all_line_numbers: bool,
    /// Show minimap.
    pub show_minimap: bool,
    /// Show status bar.
    pub show_status_bar: bool,
    /// Disable all non-essential UI motion. When true every motion
    /// contract duration resolves to zero and no tween frames are
    /// scheduled.
    pub reduced_motion: bool,
    /// Phase F1: paint the sticky heading breadcrumb at the top of
    /// every pane. Default on; toggled at runtime via the
    /// `view.toggle_sticky_breadcrumb` command.
    pub show_sticky_breadcrumb: bool,
    /// Phase F2: paint the right-docked markdown outline sidebar.
    /// Default off (heading-driven sidebar is an opt-in surface; the
    /// breadcrumb already covers the always-on heading context). Toggle
    /// at runtime via the `view.toggle_outline` command.
    pub show_outline_sidebar: bool,
    /// Phase F2: outline-sidebar width in DIPs when expanded. The
    /// renderer clamps to the pane width so a very narrow pane stays
    /// usable. Default `220`.
    pub outline_sidebar_width_dip: u32,
    /// `"always" | "hover" | "never"`.
    pub tab_close_button: String,
    /// Phase F5: upper bound on the renderer-side inline-image bitmap
    /// cache. The cache is keyed by absolute image path and bounded by
    /// total decoded bitmap bytes; when an insert would exceed this
    /// limit the cache evicts oldest-used entries until it fits.
    /// Default `67_108_864` (64 MiB). Set to `0` to disable inline
    /// image rendering entirely (equivalent to
    /// `[markdown] inline_images = false`, but expressed as a memory
    /// budget rather than a behaviour flag).
    pub image_cache_bytes: u64,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            theme_dark: "deep_minimal".into(),
            theme_light: "paper".into(),
            // Phase A §A4: gutter is always visible. Render density (caret
            // line only vs. all lines) is governed by the matching ui-side
            // `gutter_caret_line_only` flag.
            show_line_numbers: true,
            relative_line_numbers: false,
            show_all_line_numbers: false,
            show_minimap: false,
            show_status_bar: true,
            reduced_motion: false,
            show_sticky_breadcrumb: true,
            show_outline_sidebar: false,
            outline_sidebar_width_dip: 220,
            tab_close_button: "hover".into(),
            image_cache_bytes: 67_108_864,
        }
    }
}

/// `[statusbar]` section (Phase C1).
///
/// `segments` is the list of segment identifiers rendered left-to-right.
/// See [`crate::StatusBarSegment`] for the valid set.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StatusBarConfig {
    /// Ordered list of segment ids painted left-to-right.
    pub segments: Vec<String>,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            // Default segment order per §C1.
            segments: vec![
                "position".into(),
                "chars".into(),
                "words".into(),
                "lines".into(),
                "selection".into(),
                "numeric_sum".into(),
                "encoding".into(),
                "line_endings".into(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_yields_defaults() {
        let s = Settings::from_toml("").unwrap();
        assert_eq!(s.editor.font_size, 14.0);
        assert_eq!(s.persistence.debounce_ms, 300);
        assert_eq!(s.backup.interval_minutes, 15);
    }

    #[test]
    fn find_persist_per_buffer_defaults_to_true() {
        let s = Settings::from_toml("").unwrap();
        assert!(s.find.persist_per_buffer);
    }

    #[test]
    fn focus_settings_align_with_pane_modes_spec() {
        let s = Settings::from_toml("").unwrap();
        assert!((s.focus.dim_alpha - 0.45).abs() < 1e-6);
        assert_eq!(s.focus.max_column_width, 80);
        assert_eq!(s.focus.initial_mode, "off");
        assert!(!s.focus.distraction_free_on_launch);
    }

    // §H5 slash-command palette tests moved to
    // `crates/config/tests/slash_commands_palette.rs` to keep this
    // file under the 600-line cap.

    // `focus_overrides_parse` + `find_persist_per_buffer_overrides_apply`
    // tests moved to `crates/config/tests/focus_find_overrides.rs`.

    #[test]
    fn partial_overrides_apply() {
        let s = Settings::from_toml(
            r#"[editor]
font_size = 18.0
ligatures = true
mouse_wheel_scroll_speed = 1.25
"#,
        )
        .unwrap();
        assert_eq!(s.editor.font_size, 18.0);
        assert!(s.editor.ligatures);
        assert_eq!(s.editor.mouse_wheel_scroll_speed, 1.25);
        // Unspecified fields keep defaults.
        assert_eq!(s.editor.line_height, 1.35);
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(Settings::from_toml("editor = ").is_err());
    }

    #[test]
    fn indent_defaults_and_overrides_parse() {
        let s = Settings::from_toml("").unwrap();
        assert_eq!(s.editor.indent_type, "tabs");
        assert_eq!(s.editor.indent_width, 4);
        assert_eq!(s.editor.tab_width, 4);
        assert_eq!(s.indent_type(), crate::IndentType::Tabs);

        let s = Settings::from_toml(
            r#"[editor]
indent_type = "spaces"
indent_width = 2
tab_width = 8
"#,
        )
        .unwrap();
        assert_eq!(s.editor.indent_type, "spaces");
        assert_eq!(s.editor.indent_width, 2);
        assert_eq!(s.editor.tab_width, 8);
        assert_eq!(s.indent_type(), crate::IndentType::Spaces);
        // Unspecified fields keep defaults.
        assert_eq!(s.editor.font_size, 14.0);
    }

    // Smart-typography tests live in
    // `crates/config/tests/smart_typography_effective.rs`.

    #[test]
    fn parses_full_sample() {
        let s = r#"
[persistence]
mode                   = "max_safety"
debounce_ms            = 100
snapshot_every_edits   = 1000
snapshot_every_bytes   = 524288
trash_retention_days   = 90

[markdown]
reveal_mode            = "line"
heading_scale          = [3.0, 2.0, 1.5, 1.25, 1.1, 1.0]

[ui]
theme                  = "dark"
show_line_numbers      = true
"#;
        let s = Settings::from_toml(s).unwrap();
        assert_eq!(s.persistence.mode, "max_safety");
        assert_eq!(s.markdown.reveal_mode, "line");
        assert_eq!(s.markdown.heading_scale[0], 3.0);
        assert!(s.ui.show_line_numbers);
    }
}
