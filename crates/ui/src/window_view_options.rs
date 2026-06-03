//! Per-pane view-options state types: [`ViewOptions`],
//! [`StatusCountMode`], [`CaretStyle`]. The `Window` mutators that
//! flip these fields live in sibling files keyed by responsibility:
//! - `window_theme_apply.rs` — theme cycle / reload / pick / preview
//! - `window_font_picker.rs` — font family / size / ligatures
//! - `window_view_toggles.rs` — every `view.toggle_*` and ruler-columns
//!
//! Only [`Window::cycle_caret_style_impl`] stays here; it operates on
//! `view_options.caret_style` and is small enough that pulling it out
//! into its own file would cost more than it saved.
//!
//! Thread ownership: every piece of mutable state here is owned by the
//! window's UI thread.

use continuity_buffer::BufferId;

use crate::window_helpers::invalidate_hwnd;
use crate::Window;

/// Per-pane view-options state — every spec-§11 toggle plus the active
/// [`CaretStyle`]. Defaults match `settings.toml` defaults. Phase A §A4
/// flipped `line_numbers` on (the gutter is always visible) but
/// `gutter_caret_line_only` keeps render density to just the caret line.
#[derive(Debug, Clone)]
pub(crate) struct ViewOptions {
    /// Render the gutter line-number column on the left of the editor.
    pub line_numbers: bool,
    /// When the gutter is visible, render only the caret line's number
    /// (rest of the gutter stays empty). Phase A §A4 default behaviour.
    /// Set `false` to render every visible line's number Sublime-style.
    pub gutter_caret_line_only: bool,
    /// Render non-caret gutter labels as distance from the primary caret
    /// source line. The caret line itself stays absolute.
    pub relative_line_numbers: bool,
    /// Paint the current-line highlight band behind the caret line.
    pub current_line_highlight: bool,
    /// Paint vertical indent-guide rules at indent-column boundaries.
    pub indent_guides: bool,
    /// Render whitespace-marker glyphs (`·` / `→`) over space + tab runs.
    pub whitespace_markers: bool,
    /// Paint a coloured fill on trailing whitespace runs.
    pub trailing_whitespace: bool,
    /// Render the minimap (subsampled glyph-density heatmap).
    pub minimap: bool,
    /// Cached scaled-text minimap layout from the last paint, used by
    /// click and drag hit-tests.
    pub minimap_layout: Option<continuity_render::MinimapLayout>,
    /// Indent step in spaces; used to position indent guides + tab markers.
    pub indent_size: u32,
    /// On-screen width of a literal tab character, in columns. Sourced
    /// from `[editor].tab_width`; drives the DirectWrite incremental tab
    /// stop (rendered tab glyph width) and the indent-guide / whitespace
    /// tab advance. Mutated at runtime by the `editor.tab_width_*` /
    /// `editor.set_tab_width` commands.
    pub tab_width: u32,
    /// Ruler-column positions in characters (one vertical rule per column).
    pub ruler_columns: Vec<u32>,
    /// Active caret shape.
    pub caret_style: CaretStyle,
    /// Blink period for the caret in milliseconds (`0` = no blink).
    pub caret_blink_ms: u32,
    /// Phase B4: bar-mode caret width in DIPs.
    pub caret_width_px: u32,
    /// Phase B5: keep caret solid while typing; resume blink after idle.
    pub caret_blink_on_typing_pause: bool,
    /// Phase B5: idle threshold before blink resumes (ms).
    pub caret_typing_pause_ms: u32,
    /// α.3 — long-idle threshold (ms) after which the caret suspends
    /// blinking again. `0` disables the long-idle suspend.
    pub caret_long_idle_ms: u32,
    /// Phase B4: caret color override (`#rrggbb[aa]` or theme key);
    /// empty falls through to the theme's `editor.cursor.primary`.
    pub caret_color: String,
    /// Phase B4: multi-cursor secondary color override; same syntax;
    /// empty falls through to the theme.
    pub caret_secondary_color: String,
    /// Phase B7 caret motion-tween enable.
    pub caret_tween_enabled: bool,
    /// Phase B7 minimum jump (`> N`) to fire the tween.
    pub caret_tween_threshold_rows: u32,
    /// Phase B7 tween duration in ms.
    pub caret_tween_duration_ms: u32,
    /// `true` when font ligatures are enabled (DirectWrite typography).
    pub ligatures: bool,
    /// Smooth scroll on page/doc navigation. Reduced-motion overrides
    /// this to instant.
    pub smooth_scroll: bool,
    /// Allow scrolling below the last line until it can sit at the
    /// viewport top (VS Code-style overscroll). Wheel/keyboard-only —
    /// the scrollbar pins to the true content bottom and Ctrl+End still
    /// lands the last line at the viewport bottom. Sourced from
    /// `[editor].scroll_past_end`.
    pub scroll_past_end: bool,
    /// Multiplier applied to mouse-wheel line distance.
    pub mouse_wheel_scroll_speed: f32,
    /// Paint the bottom status bar (caret position + buffer dirty marker).
    pub show_status_bar: bool,
    /// Phase F1: paint the sticky heading breadcrumb at the top of
    /// each pane. Sourced from `[ui].show_sticky_breadcrumb` on
    /// settings load; toggled at runtime via
    /// `view.toggle_sticky_breadcrumb`.
    pub show_sticky_breadcrumb: bool,
    /// Phase F1 — cached breadcrumb layout from the last paint, used by
    /// the click handler to map a click rect back to a heading segment.
    pub breadcrumb_layout: Option<continuity_render::BreadcrumbLayout>,
    /// Phase F2 — paint the right-docked outline sidebar. Sourced from
    /// `[ui].show_outline_sidebar` on settings load; toggled at runtime
    /// via `view.toggle_outline`.
    pub show_outline_sidebar: bool,
    /// Phase F2 — outline-sidebar width in DIPs when expanded.
    pub outline_sidebar_width_dip: f32,
    /// Phase F2 — cached outline layout from the last paint, used by
    /// the click handler to map a click rect back to a heading row.
    pub outline_layout: Option<continuity_render::OutlineLayout>,
    /// Phase F2 — independent vertical scroll offset for the outline
    /// list. Owned by the window UI thread.
    pub outline_scroll_offset_dip: f32,
    /// Phase C1: ordered list of status-bar segments. Sourced from
    /// `[statusbar].segments` in `settings.toml`; rebuilt on every
    /// `apply_settings` call. Default matches the `StatusBarConfig`
    /// default in `continuity_config`.
    pub status_bar_segments: Vec<continuity_config::StatusBarSegment>,
    /// Phase C2: which counter the `chars` segment currently shows.
    /// Cycles `Chars → Words → Lines → Bytes` on click. Window-local
    /// state — not persisted.
    pub status_count_mode: StatusCountMode,
    /// Phase C2 — cached status-bar layout from the last paint, used
    /// by the mouse handler to hit-test segment clicks.
    pub status_bar_layout: Option<continuity_render::StatusBarLayout>,
    /// Phase G4 — cached search-active minimap layout from the last
    /// paint, used by the mouse handler to hit-test clicks on the
    /// strip and route them through `find_step`. `None` when the find
    /// bar is closed or had no matches at last paint.
    pub search_minimap_layout: Option<crate::search_minimap::MinimapLayout>,
    /// Tab close-button visibility mode.
    pub tab_close_button: continuity_config::TabCloseButton,
    /// Phase H2: paint the tab strip at the top of every pane.
    /// Defaults to `true`; distraction-free mode flips it `false`
    /// while active (and the prior state is snapshotted via
    /// [`crate::window_pane_modes::ChromeSnapshot`] for restore).
    pub show_tab_strip: bool,
    /// Phase H2: paint pane borders around every pane leaf. Defaults
    /// to `true`; distraction-free mode flips it `false` while active.
    pub show_pane_borders: bool,
    /// Phase H — focus mode (H1), distraction-free (H2), indent
    /// folding (H3), slash-palette (H5), Ctrl+Tab overlay (H6).
    pub pane_modes: crate::window_pane_modes::PaneModesState,
    /// Spec §I — time-machine slider (I1) + metrics buffer (I2).
    pub time_machine: crate::window_time_machine::TimeMachineState,
    /// Synthetic read-only buffer id for the tutorial tab in this
    /// window, once opened. `None` until the user first invokes
    /// `help.tutorial`. Reused on subsequent invocations: if the tab
    /// still exists we refocus it; if the user closed it we re-adopt
    /// the synthetic buffer as a new tab. Cleared when the tab is
    /// closed (the synthetic buffer itself remains in core's
    /// `EditorState` because closing a tab doesn't drop its buffer
    /// — same pattern as the metrics buffer).
    pub tutorial_buffer_id: Option<BufferId>,
    /// Synthetic empty buffer that backs every buffer-history tab in
    /// this window. Allocated lazily on first `view.buffer_history`
    /// dispatch (or first paint of a restored history tab) so the
    /// regular paint pipeline can run with a real `EditorSnapshot`
    /// behind the panel overlay — same pattern as
    /// [`Self::tutorial_buffer_id`] for the tutorial tab and the
    /// metrics buffer for the §I2 metrics surface.
    pub buffer_history_render_buffer_id: Option<BufferId>,
    /// `true` while the image-animation `WM_TIMER` is running. Auto-
    /// armed when an animated GIF first enters the cache; auto-
    /// disarmed when the cache drops back to all-static entries.
    pub image_animation_timer_active: bool,
    /// Spec §I1 — historical-view overlay state. `pinned_revision`
    /// tells the renderer to substitute the cached historical content
    /// for the live buffer head; cleared when the overlay dismisses.
    pub overlay: crate::view_overlay::ViewOverlay,
}

/// Phase C2: cycle position for the click-to-cycle char/word/line/byte
/// counter segment. The segment kind in the status-bar data is always
/// [`continuity_render::StatusBarSegmentKind::Chars`] regardless of the
/// concrete label — the click handler reads this state and produces the
/// matching text.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum StatusCountMode {
    /// Character (Unicode scalar) count.
    #[default]
    Chars,
    /// Word count.
    Words,
    /// Line count (non-empty / total).
    Lines,
    /// Byte count (UTF-8 octets).
    Bytes,
}

impl StatusCountMode {
    /// Advance to the next mode in cycle order.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Chars => Self::Words,
            Self::Words => Self::Lines,
            Self::Lines => Self::Bytes,
            Self::Bytes => Self::Chars,
        }
    }
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            line_numbers: true,
            gutter_caret_line_only: true,
            relative_line_numbers: false,
            current_line_highlight: false,
            indent_guides: true,
            whitespace_markers: false,
            trailing_whitespace: false,
            minimap: false,
            minimap_layout: None,
            indent_size: 4,
            tab_width: 4,
            ruler_columns: Vec::new(),
            caret_style: CaretStyle::Bar,
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
            ligatures: false,
            smooth_scroll: true,
            scroll_past_end: true,
            mouse_wheel_scroll_speed: 2.0,
            show_status_bar: true,
            show_sticky_breadcrumb: true,
            breadcrumb_layout: None,
            show_outline_sidebar: false,
            outline_sidebar_width_dip: continuity_render::OUTLINE_DEFAULT_WIDTH_DIP,
            outline_layout: None,
            outline_scroll_offset_dip: 0.0,
            // Match `StatusBarConfig::default()` so window-level defaults
            // align with the config crate before any settings load runs.
            status_bar_segments: vec![
                continuity_config::StatusBarSegment::Position,
                continuity_config::StatusBarSegment::Chars,
                continuity_config::StatusBarSegment::Words,
                continuity_config::StatusBarSegment::Lines,
                continuity_config::StatusBarSegment::Selection,
                continuity_config::StatusBarSegment::NumericSum,
                continuity_config::StatusBarSegment::Encoding,
                continuity_config::StatusBarSegment::LineEndings,
            ],
            status_count_mode: StatusCountMode::Chars,
            status_bar_layout: None,
            search_minimap_layout: None,
            tab_close_button: continuity_config::TabCloseButton::Hover,
            show_tab_strip: true,
            show_pane_borders: true,
            pane_modes: crate::window_pane_modes::PaneModesState::default(),
            time_machine: crate::window_time_machine::TimeMachineState::default(),
            tutorial_buffer_id: None,
            buffer_history_render_buffer_id: None,
            image_animation_timer_active: false,
            overlay: crate::view_overlay::ViewOverlay::default(),
        }
    }
}

/// Caret shape — spec §11 caret style.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum CaretStyle {
    /// Thin vertical bar (default).
    #[default]
    Bar,
    /// Block covering the grapheme under the caret.
    Block,
    /// Thin horizontal underline below the grapheme.
    Underline,
}

impl CaretStyle {
    /// Cycle through the three styles.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Bar => Self::Block,
            Self::Block => Self::Underline,
            Self::Underline => Self::Bar,
        }
    }
}

impl Window {
    pub(crate) fn cycle_caret_style_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.caret_style = self.view_options.caret_style.next();
        invalidate_hwnd(self.hwnd);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_cycle_walks_three_states() {
        assert_eq!(CaretStyle::default(), CaretStyle::Bar);
        assert_eq!(CaretStyle::Bar.next(), CaretStyle::Block);
        assert_eq!(CaretStyle::Block.next(), CaretStyle::Underline);
        assert_eq!(CaretStyle::Underline.next(), CaretStyle::Bar);
    }

    #[test]
    fn view_options_default_matches_settings_defaults() {
        let v = ViewOptions::default();
        // Phase A §A4: gutter is always visible by default.
        assert!(v.line_numbers);
        // Density default: only the caret line's number renders.
        assert!(v.gutter_caret_line_only);
        assert!(!v.relative_line_numbers);
        assert!(!v.minimap);
        assert!(!v.ligatures);
        assert_eq!(v.mouse_wheel_scroll_speed, 2.0);
        assert_eq!(v.indent_size, 4);
        assert_eq!(v.tab_width, 4);
        assert_eq!(v.caret_style, CaretStyle::Bar);
        assert_eq!(v.caret_blink_ms, 530);
        assert_eq!(v.caret_width_px, 2);
        assert!(v.caret_blink_on_typing_pause);
        assert_eq!(v.caret_typing_pause_ms, 400);
        assert_eq!(v.caret_long_idle_ms, 6_000);
        assert!(v.caret_color.is_empty());
        assert!(v.caret_secondary_color.is_empty());
        assert!(v.caret_tween_enabled);
        assert_eq!(v.caret_tween_threshold_rows, 5);
        assert_eq!(v.caret_tween_duration_ms, 160);
        assert!(v.smooth_scroll);
        assert!(v.scroll_past_end);
        assert!(v.ruler_columns.is_empty());
    }

    #[test]
    fn sidebar_toggles_survive_buffer_view_state_replacement() {
        let v = ViewOptions {
            minimap: true,
            show_outline_sidebar: true,
            ..Default::default()
        };

        let _incoming_buffer_view = continuity_layout::ViewState::new();

        assert!(v.minimap);
        assert!(v.show_outline_sidebar);
    }
}
