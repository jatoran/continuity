//! Goto-line and goto-heading overlay state.
//!
//! Both overlays share a single text input and a "current candidate" cursor.
//! Goto-line parses its query as `<line>` or `<line>:<col>` (1-indexed in the
//! UI; converted to 0-indexed positions by the dispatcher). Goto-heading
//! fuzzy-matches the query against the buffer's heading entries.

use continuity_decorate::HeadingEntry;
use continuity_search::{score, FuzzyMatch};

use crate::text_input::TextInput;

/// Goto-line overlay state.
#[derive(Debug, Default)]
pub struct GotoLine {
    /// Numeric input.
    pub input: TextInput,
}

impl GotoLine {
    /// A fresh goto-line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse the input as `<line>` or `<line>:<col>`. Both are 1-indexed.
    /// Returns `(line_zero_indexed, col_zero_indexed)`.
    #[must_use]
    pub fn target(&self) -> Option<(u32, u32)> {
        parse_line_col(&self.input.text)
    }
}

fn parse_line_col(s: &str) -> Option<(u32, u32)> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (line_part, col_part) = match trimmed.split_once(':') {
        Some((a, b)) => (a, Some(b)),
        None => (trimmed, None),
    };
    let line = line_part.trim().parse::<u32>().ok()?;
    let col = match col_part {
        Some(c) => c.trim().parse::<u32>().ok()?,
        None => 1,
    };
    if line == 0 || col == 0 {
        return None;
    }
    Some((line - 1, col - 1))
}

/// Goto-heading overlay state.
#[derive(Debug, Default)]
pub struct GotoHeading {
    /// Search input.
    pub input: TextInput,
    /// All headings in the active buffer (refreshed when the overlay opens).
    pub all: Vec<HeadingEntry>,
    /// Indices of filtered headings, ranked by score.
    pub filtered: Vec<usize>,
    /// Per-row fuzzy-match metadata.
    pub matches: Vec<FuzzyMatch>,
    /// Selected row within `filtered`.
    pub selected: usize,
}

impl GotoHeading {
    /// A fresh goto-heading.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the candidate list.
    pub(crate) fn set_candidates(&mut self, all: Vec<HeadingEntry>) {
        self.all = all;
        self.refilter();
    }

    /// Re-rank against the current query.
    pub(crate) fn refilter(&mut self) {
        let q = self.input.text.as_str();
        let mut scored: Vec<(usize, FuzzyMatch)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| score(q, &entry.text).map(|m| (i, m)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.score
                .cmp(&a.1.score)
                .then_with(|| self.all[a.0].line.cmp(&self.all[b.0].line))
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
    pub(crate) fn selected_entry(&self) -> Option<&HeadingEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_only() {
        assert_eq!(parse_line_col("42"), Some((41, 0)));
    }

    #[test]
    fn parses_line_col() {
        assert_eq!(parse_line_col("3:7"), Some((2, 6)));
    }

    #[test]
    fn rejects_zero() {
        assert_eq!(parse_line_col("0"), None);
        assert_eq!(parse_line_col("1:0"), None);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_line_col("abc"), None);
        assert_eq!(parse_line_col(""), None);
    }

    #[test]
    fn goto_heading_refilter_drops_non_matches() {
        let mut g = GotoHeading::new();
        g.set_candidates(vec![
            HeadingEntry {
                level: 1,
                text: "Intro".into(),
                line: 0,
                start_byte: 0,
            },
            HeadingEntry {
                level: 2,
                text: "Performance".into(),
                line: 4,
                start_byte: 10,
            },
        ]);
        g.input.set_text("perf");
        g.refilter();
        assert_eq!(g.filtered.len(), 1);
        assert_eq!(g.selected_entry().unwrap().text, "Performance");
    }
}
