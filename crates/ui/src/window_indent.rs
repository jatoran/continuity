//! Per-window indentation runtime mirror + the production
//! [`continuity_command::EditConfigContext`] impl for [`Window`].
//!
//! The mirror reflects `[editor].indent_type` / `indent_width` /
//! `tab_width`. It is the single source the indent edit path reads at
//! dispatch time (`indent_unit` / `effective_tab_width`) and the home of
//! the indent command mutators, mirroring [`crate::window_auto_pair`].
//!
//! Thread ownership: the [`IndentConfig`] mirror is owned exclusively by
//! the window's UI thread, mutated only inside
//! [`Window::apply_indent_settings`] and the command mutators below
//! (same single-writer pattern as `auto_pair` / `view_options`). Buffer
//! mutation still flows core-thread-only through `apply_selection_edit`.
//!
//! Visual tab width: [`Window::set_tab_width_impl`] /
//! [`Window::adjust_tab_width_impl`] change the on-screen width of a
//! literal tab. They drop the cached font state and reflow inside
//! [`Window::with_caret_line_anchored`] exactly like a font-size change,
//! because the rendered tab stop is derived from the font state (see
//! `render::text_metrics::measure_tab_advance_dip` +
//! `layout::FontStateId::with_tab_width`).

use continuity_config::Settings;
use continuity_core::IndentUnit;

use crate::window_helpers::invalidate_hwnd;
use crate::Window;

/// Smallest / largest indent or tab width, matching `validate.rs`.
const MIN_WIDTH: u32 = 1;
const MAX_WIDTH: u32 = 16;

/// Per-window indent configuration mirror.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct IndentConfig {
    /// `true` ⇒ indent with spaces (`indent_width` per level);
    /// `false` ⇒ indent with a tab character.
    pub use_spaces: bool,
    /// Spaces emitted per indent level when `use_spaces` is `true`.
    pub indent_width: u32,
    /// On-screen / conversion width of a literal tab, in columns.
    pub tab_width: u32,
}

impl Default for IndentConfig {
    fn default() -> Self {
        // Matches `EditorConfig::default()` (spaces / 4 / 4).
        Self {
            use_spaces: true,
            indent_width: 4,
            tab_width: 4,
        }
    }
}

impl IndentConfig {
    /// The indent unit `editor.indent` / `editor.outdent` should apply.
    fn indent_unit(self) -> IndentUnit {
        if self.use_spaces {
            IndentUnit::Spaces(self.indent_width.clamp(MIN_WIDTH, MAX_WIDTH))
        } else {
            IndentUnit::Tab
        }
    }
}

impl continuity_command::EditConfigContext for Window {
    fn auto_pair_for(&self, c: char) -> Option<(char, char)> {
        self.auto_pair.pair_for(c)
    }

    fn try_delete_back_pair(&mut self) -> Result<bool, continuity_command::Error> {
        self.try_delete_auto_pair()
    }

    fn indent_unit(&self) -> IndentUnit {
        self.indent.indent_unit()
    }

    fn effective_tab_width(&self) -> u32 {
        self.indent.tab_width.clamp(MIN_WIDTH, MAX_WIDTH)
    }
}

impl Window {
    /// Mirror the validated `[editor]` indent settings onto the
    /// per-window [`IndentConfig`] and project `indent_width` onto
    /// `view_options.indent_size` (indent-guide column spacing) and
    /// `tab_width` onto `view_options.tab_width` (rendered tab stop).
    /// Idempotent. Called from [`Window::apply_settings`].
    pub(crate) fn apply_indent_settings(&mut self, s: &Settings) {
        let use_spaces = matches!(s.indent_type(), continuity_config::IndentType::Spaces);
        let indent_width = s.editor.indent_width.clamp(MIN_WIDTH, MAX_WIDTH);
        let tab_width = s.editor.tab_width.clamp(MIN_WIDTH, MAX_WIDTH);
        let tab_width_changed = self.indent.tab_width != tab_width;
        self.indent = IndentConfig {
            use_spaces,
            indent_width,
            tab_width,
        };
        // Indent guides + tab markers use `indent_size` for spacing.
        self.view_options.indent_size = indent_width;
        // The renderer derives the literal-tab advance from this.
        self.view_options.tab_width = tab_width;
        // A tab-width change alters glyph geometry the cached layouts
        // were built against; drop them and reflow anchored on the
        // caret line, exactly like a font-size change.
        if tab_width_changed {
            self.with_caret_line_anchored(|w| w.invalidate_font_state());
        }
    }

    /// `editor.indent_use_spaces` / `editor.indent_use_tabs`.
    pub(crate) fn set_indent_type_impl(&mut self, use_spaces: bool) {
        self.indent.use_spaces = use_spaces;
        self.persist_string_or_log(
            "editor",
            "indent_type",
            if use_spaces { "spaces" } else { "tabs" },
        );
        invalidate_hwnd(self.hwnd);
    }

    /// `editor.set_indent_width` (explicit) — clamps to `1..=16`.
    pub(crate) fn set_indent_width_impl(&mut self, width: u32) {
        let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        self.indent.indent_width = width;
        self.view_options.indent_size = width;
        self.persist_int_or_log("editor", "indent_width", width);
        invalidate_hwnd(self.hwnd);
    }

    /// `editor.indent_width_increase` / `_decrease`.
    pub(crate) fn adjust_indent_width_impl(&mut self, delta: i32) {
        let next = clamp_delta(self.indent.indent_width, delta);
        self.set_indent_width_impl(next);
    }

    /// `editor.set_tab_width` (explicit) — clamps to `1..=16`. Changing
    /// the tab width changes the rendered width of a literal tab, so the
    /// font state is invalidated and the body reflows anchored on the
    /// caret line.
    pub(crate) fn set_tab_width_impl(&mut self, width: u32) {
        let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        if self.indent.tab_width != width {
            self.indent.tab_width = width;
            self.view_options.tab_width = width;
            self.with_caret_line_anchored(|w| w.invalidate_font_state());
        }
        self.persist_int_or_log("editor", "tab_width", width);
        invalidate_hwnd(self.hwnd);
    }

    /// `editor.tab_width_increase` / `_decrease`.
    pub(crate) fn adjust_tab_width_impl(&mut self, delta: i32) {
        let next = clamp_delta(self.indent.tab_width, delta);
        self.set_tab_width_impl(next);
    }
}

/// Apply a signed `delta` to a width and clamp the result to `1..=16`.
fn clamp_delta(current: u32, delta: i32) -> u32 {
    let signed = i64::from(current) + i64::from(delta);
    signed.clamp(i64::from(MIN_WIDTH), i64::from(MAX_WIDTH)) as u32
}

// The `ViewContext` indent dispatch methods (`set_indent_type`,
// `set_indent_width`, `adjust_indent_width`, `set_tab_width`,
// `adjust_tab_width`) live with the rest of `impl ViewContext for
// Window` in `crate::window_view_context`; each delegates to the
// `_impl` mutators above.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indent_config_default_is_spaces_four() {
        let c = IndentConfig::default();
        assert!(c.use_spaces);
        assert_eq!(c.indent_width, 4);
        assert_eq!(c.tab_width, 4);
        assert_eq!(c.indent_unit(), IndentUnit::Spaces(4));
    }

    #[test]
    fn indent_unit_reflects_type_and_width() {
        let spaces = IndentConfig {
            use_spaces: true,
            indent_width: 2,
            tab_width: 8,
        };
        assert_eq!(spaces.indent_unit(), IndentUnit::Spaces(2));
        let tabs = IndentConfig {
            use_spaces: false,
            indent_width: 2,
            tab_width: 8,
        };
        assert_eq!(tabs.indent_unit(), IndentUnit::Tab);
    }

    #[test]
    fn clamp_delta_stays_in_range() {
        assert_eq!(clamp_delta(4, 1), 5);
        assert_eq!(clamp_delta(1, -1), 1);
        assert_eq!(clamp_delta(16, 1), 16);
        assert_eq!(clamp_delta(1, -5), 1);
        assert_eq!(clamp_delta(16, 99), 16);
    }
}
