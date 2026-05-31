//! δ.1 — surround-on-type hook.
//!
//! When a paired open character (`(`, `[`, `` ` ``, …) is typed while
//! a non-empty selection exists AND auto-pair is enabled for that
//! character, wrap the selection in the matching pair instead of
//! replacing it with a single inserted char. The `EDITOR_SURROUND_*`
//! command family already covers manual invocation; this hook is the
//! keystroke auto-trigger that turns "type `(`" into "wrap selection
//! in `()`".
//!
//! Thread ownership: UI thread of one window — invoked from
//! `Window::on_char` before `EDITOR_INSERT_CHAR` dispatch.

use continuity_core::SelectionEdit;

use crate::Window;

impl Window {
    /// Wrap a non-empty selection in a paired character when the user
    /// types its opener. Returns `true` when the wrap fired (the
    /// caller should skip the normal insert path).
    pub(crate) fn try_surround_on_type(&mut self, ch: char) -> bool {
        let Some((open, close)) = self.auto_pair.pair_for(ch) else {
            return false;
        };
        if !self.any_selection_non_empty() {
            return false;
        }
        self.dispatch_selection_edit(SelectionEdit::SurroundSelection {
            open: open.to_string(),
            close: close.to_string(),
        })
        .is_ok()
    }

    fn any_selection_non_empty(&self) -> bool {
        let Some(snap) = self.current_snapshot() else {
            return false;
        };
        snap.selections().iter().any(|sel| !sel.is_caret())
    }
}
