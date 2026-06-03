//! Per-buffer persistence snapshot for [`crate::find_bar::FindBar`].
//!
//! The window stores one [`FindBarMemento`] per buffer so the find bar can be
//! reopened in the same buffer without retyping queries. Only user-input state
//! (query, replace, mode flags, scope) is preserved; match results and field
//! carets are scratch state recomputed on the next render.

use crate::find_bar::{FindBar, FindFocus, FindScope};

/// G2: persistent snapshot of a find bar's user-input state.
///
/// Stored per-buffer on the window; restored when the bar reopens in the
/// same buffer so the user doesn't have to retype queries.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct FindBarMemento {
    /// Saved query text.
    pub query: String,
    /// Saved replace text.
    pub replace: String,
    /// Saved replace-visible flag.
    pub replace_visible: bool,
    /// Saved `case_sensitive`.
    pub case_sensitive: bool,
    /// Saved `whole_word`.
    pub whole_word: bool,
    /// Saved `regex`.
    pub regex: bool,
    /// Saved `preserve_case`.
    pub preserve_case: bool,
    /// Saved scope.
    pub scope: FindScope,
}

impl FindBar {
    /// G2: snapshot the bar's user-input state for per-buffer restoration.
    /// Match results and field carets are NOT preserved — they're scratch
    /// state that gets recomputed on the next render.
    #[must_use]
    pub(crate) fn to_memento(&self) -> FindBarMemento {
        FindBarMemento {
            query: self.query().to_owned(),
            replace: self.replace().to_owned(),
            replace_visible: self.replace_visible,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            preserve_case: self.preserve_case,
            scope: self.scope,
        }
    }

    /// G2: build a fresh find bar populated from `m`. Caret lands at the end
    /// of each field so a follow-up Backspace clears the last char rather
    /// than starting from a mid-string position.
    #[must_use]
    pub(crate) fn from_memento(m: &FindBarMemento) -> Self {
        let mut bar = Self {
            focus: FindFocus::Find,
            replace_visible: m.replace_visible,
            case_sensitive: m.case_sensitive,
            whole_word: m.whole_word,
            regex: m.regex,
            preserve_case: m.preserve_case,
            matches: Vec::new(),
            current: 0,
            matches_revision: 0,
            scope: m.scope,
            ..Self::default()
        };
        bar.query_input.set_text(m.query.clone());
        bar.replace_input.set_text(m.replace.clone());
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_search::MatchRange;

    #[test]
    fn memento_round_trips_user_input_state() {
        let mut f = FindBar::new();
        f.query_input.set_text("hello");
        f.replace_input.set_text("world");
        f.replace_visible = true;
        f.case_sensitive = true;
        f.whole_word = true;
        f.regex = true;
        f.scope = FindScope::Selection;
        f.matches.push(MatchRange {
            line: 1,
            start_byte: 0,
            end_byte: 1,
        });
        f.current = 7;

        let m = f.to_memento();
        let restored = FindBar::from_memento(&m);
        assert_eq!(restored.query(), "hello");
        assert_eq!(restored.replace(), "world");
        assert!(restored.replace_visible);
        assert!(restored.case_sensitive);
        assert!(restored.whole_word);
        assert!(restored.regex);
        assert_eq!(restored.scope, FindScope::Selection);
        // Match results are scratch — recomputed by the window, not restored.
        assert!(restored.matches.is_empty());
        assert_eq!(restored.current, 0);
        // Caret lands at end-of-string so editing extends rather than splits.
        assert_eq!(restored.query_caret(), 5);
        assert_eq!(restored.replace_caret(), 5);
    }
}
