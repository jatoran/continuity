//! Find-bar state: query, replace, modes, and the current match cursor.
//!
//! Pure data; the window owns one of these inside [`crate::overlays::Overlays`]
//! and drives match navigation by calling `set_results`. The find-bar is the
//! authoritative source for "what's the active query and which match index is
//! highlighted right now"; the renderer reads from it.
//!
//! The two text fields (`query`, `replace`) are each a
//! [`crate::text_input::TextInput`] — that's the same type backing every
//! other overlay's input, which means the overlay routing layer can hand the
//! find bar the *focused* `TextInput` via [`FindBar::focused_input_mut`] and
//! service editing chords (Ctrl+A/C/X/V, Shift+Home/End/arrows, click-to-set-
//! caret) without find-bar-specific paths.

use continuity_buffer::BufferId;
use continuity_search::MatchRange;

use crate::find_regex_help::FindControl;
use crate::pane_tree::PaneId;
use crate::text_input::TextInput;

/// G2: scope a find / replace operation can target.
///
/// `Buffer` is the full buffer (default). `Selection` restricts the match
/// set to the byte ranges that are currently selected; the find-bar still
/// runs the query but matches outside the selection drop out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum FindScope {
    /// Whole-buffer match. Default.
    #[default]
    Buffer,
    /// Selection-only match.
    Selection,
}

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

/// Which input field has caret focus.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum FindFocus {
    /// Search query field.
    #[default]
    Find,
    /// Replace field.
    Replace,
}

/// Focused pane/buffer snapshot that produced the current match set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct FindTarget {
    pub pane_id: PaneId,
    pub buffer_id: BufferId,
    pub revision: u64,
}

/// Find/replace bar state.
#[derive(Debug, Default)]
pub struct FindBar {
    /// Search query input (text + caret + selection anchor).
    pub query_input: TextInput,
    /// Replace input (text + caret + selection anchor).
    pub replace_input: TextInput,
    /// Which input has caret focus.
    pub focus: FindFocus,
    /// `true` when the replace field is visible.
    pub replace_visible: bool,
    /// `true` for case-sensitive matching.
    pub case_sensitive: bool,
    /// `true` for whole-word matching.
    pub whole_word: bool,
    /// `true` to interpret `query` as a regex; otherwise the query is escaped.
    pub regex: bool,
    /// `true` to adapt replacement case to each matched span.
    pub preserve_case: bool,
    /// Most recent set of matches (over the current buffer's snapshot).
    pub matches: Vec<MatchRange>,
    /// Index of the currently-highlighted match.
    pub current: usize,
    /// Pane/buffer/revision that produced `matches`.
    pub(crate) target: Option<FindTarget>,
    /// Human target label shown in the find footer.
    pub(crate) target_label: String,
    /// Last revision the matches were computed against. Used to invalidate.
    pub matches_revision: u64,
    /// Phase G2: match scope — full buffer (default) or selection-only.
    pub scope: FindScope,
    /// δ.3 — last regex compile error, if the query failed to compile.
    /// `Some(msg)` puts the bar's X-of-N counter into an error mode so
    /// the user can see WHY there are no matches; cleared on every
    /// successful (or non-regex) recompute.
    pub regex_error: Option<String>,
    /// Find-bar control currently under the pointer.
    pub(crate) hovered_control: Option<FindControl>,
    /// Byte ranges captured for selection-scoped search.
    pub(crate) selection_scope_ranges: Vec<(usize, usize)>,
}

impl FindBar {
    /// A fresh, empty find bar.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A find bar with replace pre-shown.
    #[must_use]
    pub fn with_replace() -> Self {
        Self {
            replace_visible: true,
            ..Self::default()
        }
    }

    /// The search query text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query_input.text
    }

    /// The caret byte offset within the search query.
    #[must_use]
    pub fn query_caret(&self) -> usize {
        self.query_input.caret
    }

    /// The replace text.
    #[must_use]
    pub fn replace(&self) -> &str {
        &self.replace_input.text
    }

    /// The caret byte offset within the replace text.
    #[must_use]
    pub fn replace_caret(&self) -> usize {
        self.replace_input.caret
    }

    /// Borrow the focused field's text + caret (read-only).
    #[must_use]
    pub fn active_field(&self) -> (&str, usize) {
        let input = match self.focus {
            FindFocus::Find => &self.query_input,
            FindFocus::Replace => &self.replace_input,
        };
        (&input.text, input.caret)
    }

    /// Mutably borrow the focused `TextInput`. The overlay routing layer
    /// uses this to service editing chords (Ctrl+A/C/X/V, Shift+arrows,
    /// click-to-set-caret) without going through the per-op helpers below.
    pub fn focused_input_mut(&mut self) -> &mut TextInput {
        match self.focus {
            FindFocus::Find => &mut self.query_input,
            FindFocus::Replace => &mut self.replace_input,
        }
    }

    /// Insert `c` at the focused caret.
    pub fn insert_char(&mut self, c: char) {
        self.focused_input_mut().insert_char(c);
    }

    /// Delete the byte before the focused caret (UTF-8 aware).
    pub fn delete_back(&mut self) -> bool {
        self.focused_input_mut().delete_back()
    }

    /// Delete the byte after the focused caret (UTF-8 aware).
    pub fn delete_forward(&mut self) -> bool {
        self.focused_input_mut().delete_forward()
    }

    /// Move the focused caret left one character.
    pub(crate) fn move_left(&mut self) -> bool {
        self.focused_input_mut().move_left()
    }

    /// Move the focused caret right one character.
    pub(crate) fn move_right(&mut self) -> bool {
        self.focused_input_mut().move_right()
    }

    /// Move the focused caret to start.
    pub(crate) fn move_home(&mut self) {
        self.focused_input_mut().move_home();
    }

    /// Move the focused caret to end.
    pub(crate) fn move_end(&mut self) {
        self.focused_input_mut().move_end();
    }

    /// Toggle focus between query and replace.
    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FindFocus::Find => FindFocus::Replace,
            FindFocus::Replace => FindFocus::Find,
        };
    }

    /// Apply the mode explicitly requested by `editor.find` /
    /// `editor.replace`, independent of restored input history.
    pub(crate) fn apply_requested_find_mode(&mut self, with_replace: bool) {
        self.replace_visible = with_replace;
        self.focus = if with_replace {
            FindFocus::Replace
        } else {
            FindFocus::Find
        };
    }

    /// Set match results and clamp `current`.
    pub(crate) fn set_results(&mut self, matches: Vec<MatchRange>, revision: u64) {
        self.matches = matches;
        self.matches_revision = revision;
        if self.matches.is_empty() || self.current >= self.matches.len() {
            self.current = 0;
        }
    }

    /// Set match results for a specific pane/buffer target.
    pub(crate) fn set_results_for_target(&mut self, matches: Vec<MatchRange>, target: FindTarget) {
        let target_changed = match self.target {
            Some(current) => {
                current.pane_id != target.pane_id || current.buffer_id != target.buffer_id
            }
            None => true,
        };
        self.set_results(matches, target.revision);
        if target_changed {
            self.current = 0;
        }
        self.target = Some(target);
    }

    /// `true` when the current match set was built for `target`.
    #[must_use]
    pub(crate) fn matches_target(&self, target: FindTarget) -> bool {
        self.target == Some(target)
    }

    /// Step `delta` matches forward (negative = backward), wrapping.
    pub fn step(&mut self, delta: i32) {
        if self.matches.is_empty() {
            return;
        }
        let len = self.matches.len() as i32;
        let mut i = self.current as i32 + delta;
        i = i.rem_euclid(len);
        self.current = i as usize;
    }

    /// Currently-highlighted match, if any.
    #[must_use]
    pub(crate) fn current_match(&self) -> Option<&MatchRange> {
        self.matches.get(self.current)
    }

    /// G1: human label for the live X-of-N counter.
    ///
    /// Returns `""` while the query is empty (the bar shows nothing
    /// then). δ.3 — when `regex_error` is set, returns that message so
    /// "invalid regex" is visually distinct from "no matches".
    /// Otherwise: `"no matches"` when the query is non-empty but
    /// unmatched, or `"match N of M"` (1-indexed).
    #[must_use]
    pub fn match_label(&self) -> String {
        if self.query().is_empty() {
            return String::new();
        }
        if let Some(err) = self.regex_error.as_deref() {
            return err.to_string();
        }
        if self.matches.is_empty() {
            return "no matches".to_string();
        }
        format!("match {} of {}", self.current + 1, self.matches.len())
    }

    /// G1: flip `case_sensitive`. Caller re-runs `recompute_find_matches`.
    pub(crate) fn toggle_case_sensitive(&mut self) {
        self.case_sensitive = !self.case_sensitive;
    }

    /// G1: flip `whole_word`. Caller re-runs `recompute_find_matches`.
    pub(crate) fn toggle_whole_word(&mut self) {
        self.whole_word = !self.whole_word;
    }

    /// G1: flip `regex`. Caller re-runs `recompute_find_matches`.
    pub(crate) fn toggle_regex(&mut self) {
        self.regex = !self.regex;
    }

    /// G1: flip `preserve_case`.
    pub(crate) fn toggle_preserve_case(&mut self) {
        self.preserve_case = !self.preserve_case;
    }

    /// G2: cycle scope `Buffer ↔ Selection`. Caller re-runs matches.
    pub fn toggle_scope(&mut self) {
        self.scope = match self.scope {
            FindScope::Buffer => FindScope::Selection,
            FindScope::Selection => FindScope::Buffer,
        };
    }

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

    #[test]
    fn insert_and_caret_track_bytes() {
        let mut f = FindBar::new();
        f.insert_char('a');
        f.insert_char('b');
        assert_eq!(f.query(), "ab");
        assert_eq!(f.query_caret(), 2);
    }

    #[test]
    fn delete_back_handles_multibyte() {
        let mut f = FindBar::new();
        f.insert_char('é'); // 2 bytes utf-8
        assert_eq!(f.query().len(), 2);
        assert!(f.delete_back());
        assert!(f.query().is_empty());
        assert_eq!(f.query_caret(), 0);
    }

    #[test]
    fn move_caret_clamps_at_bounds() {
        let mut f = FindBar::new();
        assert!(!f.move_left());
        f.insert_char('x');
        assert!(f.move_left());
        assert_eq!(f.query_caret(), 0);
        assert!(f.move_right());
        assert_eq!(f.query_caret(), 1);
        assert!(!f.move_right());
    }

    #[test]
    fn toggle_focus_swaps_field() {
        let mut f = FindBar::new();
        assert_eq!(f.focus, FindFocus::Find);
        f.toggle_focus();
        assert_eq!(f.focus, FindFocus::Replace);
        f.insert_char('z');
        assert_eq!(f.replace(), "z");
    }

    #[test]
    fn focused_input_mut_targets_replace_when_focused() {
        let mut f = FindBar::new();
        f.focus = FindFocus::Replace;
        f.focused_input_mut().insert_char('q');
        assert_eq!(f.replace(), "q");
        assert_eq!(f.query(), "");
    }

    #[test]
    fn requested_find_mode_overrides_replace_visibility() {
        let mut f = FindBar::with_replace();
        f.apply_requested_find_mode(false);
        assert!(!f.replace_visible);
        assert_eq!(f.focus, FindFocus::Find);
        f.apply_requested_find_mode(true);
        assert!(f.replace_visible);
        assert_eq!(f.focus, FindFocus::Replace);
    }

    #[test]
    fn select_all_via_focused_input_then_replace() {
        let mut f = FindBar::new();
        f.query_input.set_text("hello");
        f.focused_input_mut().select_all();
        f.insert_char('x');
        assert_eq!(f.query(), "x");
        assert_eq!(f.query_caret(), 1);
    }

    #[test]
    fn step_wraps_around() {
        let mut f = FindBar::new();
        f.matches = vec![
            MatchRange {
                line: 1,
                start_byte: 0,
                end_byte: 1,
            },
            MatchRange {
                line: 1,
                start_byte: 2,
                end_byte: 3,
            },
            MatchRange {
                line: 1,
                start_byte: 4,
                end_byte: 5,
            },
        ];
        f.step(1);
        assert_eq!(f.current, 1);
        f.step(2);
        assert_eq!(f.current, 0);
        f.step(-1);
        assert_eq!(f.current, 2);
    }

    #[test]
    fn match_label_empty_query() {
        let f = FindBar::new();
        assert_eq!(f.match_label(), "");
    }

    #[test]
    fn match_label_no_matches() {
        let mut f = FindBar::new();
        f.insert_char('x');
        assert_eq!(f.match_label(), "no matches");
    }

    /// δ.3 — regex_error replaces the "no matches" label so the user
    /// can see WHY there's no result, distinct from a legitimate
    /// zero-match.
    #[test]
    fn match_label_shows_regex_error_distinct_from_no_matches() {
        let mut f = FindBar::new();
        f.insert_char('(');
        f.regex_error = Some("invalid regex: unclosed group".to_string());
        assert_eq!(f.match_label(), "invalid regex: unclosed group");
    }

    #[test]
    fn match_label_uses_one_indexed_counter() {
        let mut f = FindBar::new();
        f.insert_char('a');
        f.set_results(
            vec![
                MatchRange {
                    line: 1,
                    start_byte: 0,
                    end_byte: 1,
                },
                MatchRange {
                    line: 1,
                    start_byte: 2,
                    end_byte: 3,
                },
                MatchRange {
                    line: 1,
                    start_byte: 4,
                    end_byte: 5,
                },
            ],
            7,
        );
        assert_eq!(f.match_label(), "match 1 of 3");
        f.step(1);
        assert_eq!(f.match_label(), "match 2 of 3");
        f.step(2);
        assert_eq!(f.match_label(), "match 1 of 3");
    }

    #[test]
    fn toggles_flip_independent_flags() {
        let mut f = FindBar::new();
        assert!(!f.case_sensitive && !f.whole_word && !f.regex);
        f.toggle_case_sensitive();
        f.toggle_whole_word();
        f.toggle_regex();
        assert!(f.case_sensitive && f.whole_word && f.regex);
        f.toggle_case_sensitive();
        assert!(!f.case_sensitive && f.whole_word && f.regex);
    }

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

    #[test]
    fn toggle_scope_cycles_buffer_to_selection() {
        let mut f = FindBar::new();
        assert_eq!(f.scope, FindScope::Buffer);
        f.toggle_scope();
        assert_eq!(f.scope, FindScope::Selection);
        f.toggle_scope();
        assert_eq!(f.scope, FindScope::Buffer);
    }

    #[test]
    fn set_results_clamps_current() {
        let mut f = FindBar::new();
        f.current = 5;
        f.set_results(
            vec![MatchRange {
                line: 1,
                start_byte: 0,
                end_byte: 1,
            }],
            7,
        );
        assert_eq!(f.current, 0);
    }
}
