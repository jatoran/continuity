//! Quick-open buffer switcher state.
//!
//! Holds the search input, the per-buffer summaries the palette knows about,
//! and a fuzzy-ranked filtered list. The window populates `all` from
//! [`continuity_core::EditorHandle::list_buffers`] each time the overlay is
//! opened.

use continuity_buffer::BufferId;
use continuity_search::{score, FuzzyMatch};

use crate::text_input::TextInput;

/// Display row for one buffer.
#[derive(Clone, Debug)]
pub struct QuickOpenEntry {
    /// Underlying buffer id.
    pub id: BufferId,
    /// Display title (derived from first non-empty line, or `Untitled`).
    pub title: String,
    /// First non-empty line — used as the secondary, lower-priority match key.
    pub first_line: String,
}

/// Quick-open state.
#[derive(Debug, Default)]
pub struct QuickOpen {
    /// Search input.
    pub input: TextInput,
    /// Full candidate list.
    pub all: Vec<QuickOpenEntry>,
    /// Indices into `all` of currently-shown matches, in score order.
    pub filtered: Vec<usize>,
    /// Per-filtered-row fuzzy-match metadata (matched against the title).
    pub matches: Vec<FuzzyMatch>,
    /// Selected row within `filtered`.
    pub selected: usize,
}

impl QuickOpen {
    /// A fresh quick-open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the candidate list.
    pub(crate) fn set_candidates(&mut self, all: Vec<QuickOpenEntry>) {
        self.all = all;
        self.refilter();
    }

    /// Re-rank against the current query. Title score is primary; first-line
    /// match contributes a small bonus.
    pub(crate) fn refilter(&mut self) {
        let q = self.input.text.as_str();
        let mut scored: Vec<(usize, FuzzyMatch)> = Vec::new();
        for (i, entry) in self.all.iter().enumerate() {
            let title_score = score(q, &entry.title);
            let line_score = score(q, &entry.first_line);
            match (title_score, line_score) {
                (Some(t), Some(l)) => {
                    let combined = FuzzyMatch {
                        score: t.score + l.score / 4,
                        matched_indices: t.matched_indices,
                    };
                    scored.push((i, combined));
                }
                (Some(t), None) => scored.push((i, t)),
                (None, Some(l)) => scored.push((
                    i,
                    FuzzyMatch {
                        score: l.score / 2,
                        matched_indices: Vec::new(),
                    },
                )),
                (None, None) => {}
            }
        }
        scored.sort_by(|a, b| {
            b.1.score
                .cmp(&a.1.score)
                .then_with(|| self.all[a.0].title.cmp(&self.all[b.0].title))
        });
        self.filtered.clear();
        self.matches.clear();
        for (i, m) in scored {
            self.filtered.push(i);
            self.matches.push(m);
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    /// Move the selection cursor by `delta` rows.
    pub fn step(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as i32;
        let next = (self.selected as i32 + delta).max(0).min(len - 1);
        self.selected = next as usize;
    }

    /// Currently-selected entry.
    #[must_use]
    pub(crate) fn selected_entry(&self) -> Option<&QuickOpenEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, first_line: &str) -> QuickOpenEntry {
        QuickOpenEntry {
            id: BufferId::new(),
            title: title.into(),
            first_line: first_line.into(),
        }
    }

    #[test]
    fn empty_query_lists_all() {
        let mut q = QuickOpen::new();
        q.set_candidates(vec![entry("alpha", ""), entry("beta", "")]);
        assert_eq!(q.filtered.len(), 2);
    }

    #[test]
    fn title_match_outranks_first_line_match() {
        let mut q = QuickOpen::new();
        q.set_candidates(vec![entry("alpha", "rust here"), entry("rust", "alpha")]);
        q.input.set_text("rust");
        q.refilter();
        assert_eq!(q.selected_entry().unwrap().title, "rust");
    }

    #[test]
    fn step_clamps() {
        let mut q = QuickOpen::new();
        q.set_candidates(vec![entry("a", ""), entry("b", "")]);
        q.step(-3);
        assert_eq!(q.selected, 0);
        q.step(50);
        assert_eq!(q.selected, 1);
    }
}
