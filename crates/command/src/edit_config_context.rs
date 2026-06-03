//! `EditConfigContext` — the editing-policy surface (auto-pair lookups
//! plus the live indent configuration), factored out of [`crate::Context`]
//! so the latter stays under the 600-line cap.
//!
//! These methods answer "how should typed input be shaped?" without
//! mutating buffer state: which characters auto-pair, whether backspace
//! should delete an empty pair, what indent unit `editor.indent` should
//! insert, and how wide a tab is for `editor.spaces_to_tabs` /
//! `tabs_to_spaces`. The production impl lives on `ui::Window`, which
//! reads its per-window indent mirror; test stubs inherit the defaults
//! (`Tab` unit / width `4`) so they need not implement anything.

use continuity_core::IndentUnit;

use crate::Error;

/// Editing-policy lookups (supertrait of [`crate::Context`]).
pub trait EditConfigContext {
    /// Phase-16.5 auto-pair lookup. `None` ⇒ plain insert.
    fn auto_pair_for(&self, _c: char) -> Option<(char, char)> {
        None
    }

    /// Phase-16.5 backspace-aware delete-pair. `Ok(true)` ⇒ pair
    /// deleted; `Ok(false)` ⇒ fall through to delete_back. Errors
    /// propagate from the underlying pair-delete plan.
    ///
    /// # Errors
    ///
    /// Returns any core error from the underlying pair-delete plan.
    fn try_delete_back_pair(&mut self) -> Result<bool, Error> {
        Ok(false)
    }

    /// The indent unit `editor.indent` / `editor.outdent` should apply,
    /// read live at dispatch time. Defaults to [`IndentUnit::Tab`] so
    /// stub contexts behave as the editor did before indent settings
    /// existed. The production impl returns `Spaces(indent_width)` when
    /// `[editor].indent_type = "spaces"`, else `Tab`.
    fn indent_unit(&self) -> IndentUnit {
        IndentUnit::Tab
    }

    /// The tab width (in columns) that `editor.spaces_to_tabs` /
    /// `editor.tabs_to_spaces` should use, read live at dispatch time.
    /// Defaults to `4` to match the pre-settings constant. The production
    /// impl returns `[editor].tab_width`.
    fn effective_tab_width(&self) -> u32 {
        4
    }
}
