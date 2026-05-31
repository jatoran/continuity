//! Font-picker overlay (§E3).
//!
//! Opens as a palette-mode picker that lists every installed Windows font
//! family (DirectWrite system font collection). Moving the highlight
//! re-renders the editor body in the highlighted family **without
//! committing** — Enter writes the choice through and Esc reverts to the
//! family in effect when the palette opened.
//!
//! State only — Win32 / DirectWrite enumeration is dispatched through
//! [`continuity_layout::DWriteFactory::system_font_families`], and live
//! preview is driven by `crate::Window::set_font_family` from the
//! confirm / step / dismiss paths.
//!
//! Thread ownership: UI thread of one window. Mutated only by the
//! `Window` that owns the overlay.

use continuity_search::{score, FuzzyMatch};

use crate::text_input::TextInput;

/// Live state for an open font picker.
#[derive(Debug)]
pub struct FontPicker {
    /// Filter input shown at the top of the palette panel.
    pub input: TextInput,
    /// All discovered family names (case-insensitively sorted).
    pub all: Vec<String>,
    /// Indices into `all` of the currently-shown matches, in score order.
    pub filtered: Vec<usize>,
    /// Per-filtered-row fuzzy-match metadata.
    pub matches: Vec<FuzzyMatch>,
    /// Selected row within `filtered`.
    pub selected: usize,
    /// Family that was in effect when the picker opened. Esc reverts to
    /// this; an Enter on a different family replaces it.
    pub original_family: String,
    /// Last family applied as a *preview*. Used to avoid redundant
    /// `set_font_family` calls when `set_query` updates the filter list
    /// but keeps the same row highlighted (§E3: throttle preview swap to
    /// once per highlight change, not per filter-line keystroke).
    pub last_previewed: Option<String>,
}

impl FontPicker {
    /// Open a fresh picker preloaded with `all` system families. `original`
    /// is the family currently shown by the editor — Esc reverts here.
    #[must_use]
    pub fn new(all: Vec<String>, original: String) -> Self {
        let mut picker = Self {
            input: TextInput::default(),
            all,
            filtered: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            original_family: original.clone(),
            last_previewed: None,
        };
        picker.refilter();
        // Anchor the selection on the original family when present so
        // the first preview is a no-op.
        if let Some(idx) = picker
            .filtered
            .iter()
            .position(|i| picker.all[*i] == original)
        {
            picker.selected = idx;
        }
        picker.last_previewed = Some(original);
        picker
    }

    /// Re-rank `all` against the current filter line. Preserves the
    /// selected family by label when possible — the §E3 throttling rule
    /// hinges on `selected_family()` only changing on an actual highlight
    /// move, not on every keystroke.
    pub(crate) fn refilter(&mut self) {
        let q = self.input.text.as_str();
        let prev_label = self
            .filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
            .cloned();
        let mut scored: Vec<(usize, FuzzyMatch)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, name)| score(q, name).map(|m| (i, m)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.score.cmp(&a.1.score).then_with(|| {
                self.all[a.0]
                    .to_ascii_lowercase()
                    .cmp(&self.all[b.0].to_ascii_lowercase())
            })
        });
        self.filtered.clear();
        self.matches.clear();
        for (i, m) in scored {
            self.filtered.push(i);
            self.matches.push(m);
        }
        // Preserve selection by label when possible; otherwise reset to
        // the top row (so a query that no longer matches the previously
        // highlighted family lands on the best-scoring match).
        if let Some(label) = prev_label {
            if let Some(pos) = self.filtered.iter().position(|i| self.all[*i] == label) {
                self.selected = pos;
                return;
            }
        }
        self.selected = 0;
    }

    /// Move the highlight by `delta`, clamping.
    pub fn step(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as i32;
        let next = (self.selected as i32 + delta).max(0).min(len - 1);
        self.selected = next as usize;
    }

    /// Currently-highlighted family, if any.
    #[must_use]
    pub(crate) fn selected_family(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
            .map(String::as_str)
    }

    /// Decide whether `selected_family` is a new highlight that warrants
    /// re-applying the preview. Returns `Some(family)` for the caller to
    /// apply, or `None` when the same family is already previewed.
    pub(crate) fn next_preview_family(&mut self) -> Option<String> {
        let candidate = self.selected_family()?.to_string();
        if self.last_previewed.as_deref() == Some(candidate.as_str()) {
            return None;
        }
        self.last_previewed = Some(candidate.clone());
        Some(candidate)
    }

    /// Family Esc should revert to.
    #[must_use]
    pub fn revert_family(&self) -> &str {
        &self.original_family
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn families() -> Vec<String> {
        vec![
            "Arial".into(),
            "Cascadia Code".into(),
            "Cascadia Mono".into(),
            "Consolas".into(),
            "Segoe UI".into(),
            "Segoe UI Variable".into(),
        ]
    }

    #[test]
    fn new_anchors_on_original_family() {
        let p = FontPicker::new(families(), "Consolas".into());
        assert_eq!(p.selected_family(), Some("Consolas"));
        // No preview yet — last_previewed == original, so next_preview
        // returns None.
        let mut p = p;
        assert_eq!(p.next_preview_family(), None);
    }

    #[test]
    fn original_unknown_anchors_to_first_row() {
        let p = FontPicker::new(families(), "NotInstalled".into());
        assert_eq!(p.selected_family(), Some("Arial"));
    }

    #[test]
    fn refilter_keeps_selection_by_label_when_possible() {
        let mut p = FontPicker::new(families(), "Consolas".into());
        p.input.set_text("Cascadia");
        p.refilter();
        // "Consolas" no longer matches → selection clamps to first
        // matching row.
        assert_eq!(p.selected_family(), Some("Cascadia Code"));
    }

    #[test]
    fn refilter_holds_selection_when_family_still_matches() {
        let mut p = FontPicker::new(families(), "Cascadia Mono".into());
        p.input.set_text("Cascadia");
        p.refilter();
        assert_eq!(p.selected_family(), Some("Cascadia Mono"));
    }

    #[test]
    fn step_clamps_to_bounds() {
        let mut p = FontPicker::new(families(), "Consolas".into());
        p.input.set_text("Cascadia");
        p.refilter();
        p.step(-50);
        assert_eq!(p.selected_family(), Some("Cascadia Code"));
        p.step(50);
        assert_eq!(p.selected_family(), Some("Cascadia Mono"));
    }

    #[test]
    fn next_preview_throttles_to_one_per_highlight_change() {
        let mut p = FontPicker::new(families(), "Consolas".into());
        // Same row → no work.
        assert_eq!(p.next_preview_family(), None);
        p.step(1);
        let v = p.next_preview_family();
        assert!(v.is_some());
        // Idempotent on a second call without stepping.
        assert_eq!(p.next_preview_family(), None);
    }

    #[test]
    fn revert_family_holds_original() {
        let p = FontPicker::new(families(), "Consolas".into());
        assert_eq!(p.revert_family(), "Consolas");
    }

    #[test]
    fn empty_filter_keeps_full_list() {
        let p = FontPicker::new(families(), "Arial".into());
        assert_eq!(p.filtered.len(), families().len());
    }
}
