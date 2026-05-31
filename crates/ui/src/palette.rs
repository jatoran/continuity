//! Command palette state.
//!
//! The palette holds a query, the ranked candidate list, and a selection
//! cursor. Each candidate carries the originating command name and an
//! "applicable" flag (false → predicate-grayed, dispatch is blocked).
//!
//! δ.2 — palette ranking favours recently-used commands. A monotonic
//! counter (`recency_tick`) is bumped every time a palette dispatch
//! commits; the chosen command's `last_used` is stamped to the new
//! value. `refilter()` then sorts by score first (fuzzy match still
//! wins) and uses `last_used` as the next tiebreaker so muscle memory
//! beats alphabetical when scores are equal — which is the common
//! "empty query" case and the early-typing case before the score has
//! pulled candidates apart. State is in-memory only; it resets when
//! the window closes.

use std::collections::HashMap;

use continuity_search::FuzzyMatch;

use crate::palette_math::{self, MathPreview};
use crate::palette_rank::score_entry;
use crate::text_input::TextInput;

/// Maximum command-palette rows visible before list scrolling takes over.
pub(crate) const PALETTE_VISIBLE_ROW_LIMIT: usize = 10;

/// One palette candidate.
#[derive(Clone, Debug)]
pub struct PaletteEntry {
    /// The command name (`editor.find`, `markdown.toggle_bold`, …).
    pub command: String,
    /// Optional displayed binding (`Ctrl+F`).
    pub keybinding: Option<String>,
    /// Optional one-line command description.
    pub description: Option<String>,
    /// `false` when the active context predicate didn't apply; the row is
    /// rendered grayed-out and Enter is suppressed.
    pub applicable: bool,
}

/// Command palette state.
#[derive(Debug, Default)]
pub struct Palette {
    /// Search input.
    pub input: TextInput,
    /// Full candidate list, in registration order.
    pub all: Vec<PaletteEntry>,
    /// Indices into `all` of the currently-shown matches, in score order.
    pub filtered: Vec<usize>,
    /// Per-filtered-row fuzzy-match metadata.
    pub matches: Vec<FuzzyMatch>,
    /// Selected row in the virtual row list. When [`Self::math_preview`]
    /// is `Some`, index 0 is the math result; otherwise index 0 is the
    /// first filtered command. Always clamped to [0, `Self::row_count`).
    pub selected: usize,
    /// First virtual row currently visible in the capped result window.
    /// Mutated only on the UI thread that owns the containing window.
    pub(crate) first_visible: usize,
    /// §E2 — when the filter line parses as an arithmetic expression,
    /// the evaluated preview is held here and surfaces as a synthetic
    /// row at the top of the result list.
    pub math_preview: Option<MathPreview>,
    /// δ.2 — last-used tick per command id. Higher = more recently
    /// dispatched. Only populated for commands the user actually fires
    /// through the palette so the ranking adapts to behaviour rather
    /// than to ambient command registrations.
    pub(crate) last_used: HashMap<String, u64>,
    /// δ.2 — monotonic counter; bumped by [`Self::note_command_used`]
    /// before stamping `last_used`. Starts at 0 (never compared, so
    /// the initial state's "never used" candidates all tie at 0).
    pub(crate) recency_tick: u64,
}

impl Palette {
    /// A fresh palette.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the candidate list (e.g. on every show).
    pub(crate) fn set_candidates(&mut self, all: Vec<PaletteEntry>) {
        self.all = all;
        self.refilter();
    }

    /// Re-rank `all` against the current query.
    pub(crate) fn refilter(&mut self) {
        let q = self.input.text.as_str();
        let mut scored: Vec<(usize, FuzzyMatch)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| score_entry(q, entry).map(|m| (i, m)))
            .collect();
        let is_empty_query = q.trim().is_empty();
        scored.sort_by(|a, b| {
            if is_empty_query {
                let recency_a = self.recency(&self.all[a.0].command);
                let recency_b = self.recency(&self.all[b.0].command);
                let recency_cmp = recency_b.cmp(&recency_a);
                if recency_cmp != std::cmp::Ordering::Equal {
                    return recency_cmp;
                }
            }
            let score_cmp = b.1.score.cmp(&a.1.score);
            if score_cmp != std::cmp::Ordering::Equal {
                return score_cmp;
            }
            if !is_empty_query {
                let recency_a = self.recency(&self.all[a.0].command);
                let recency_b = self.recency(&self.all[b.0].command);
                let recency_cmp = recency_b.cmp(&recency_a);
                if recency_cmp != std::cmp::Ordering::Equal {
                    return recency_cmp;
                }
            }
            self.all[a.0].command.cmp(&self.all[b.0].command)
        });
        self.filtered.clear();
        self.matches.clear();
        for (i, m) in scored {
            self.filtered.push(i);
            self.matches.push(m);
        }
        self.math_preview = palette_math::preview(q);
        let total = self.row_count();
        if total == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(total - 1);
        }
        self.clamp_first_visible();
        self.ensure_selected_visible();
    }

    /// δ.2 — record that `command` was just dispatched through the
    /// palette. Bumps the monotonic tick so the next `refilter` lifts
    /// this command above same-score peers.
    pub(crate) fn note_command_used(&mut self, command: &str) {
        self.recency_tick = self.recency_tick.saturating_add(1);
        self.last_used
            .insert(command.to_string(), self.recency_tick);
    }

    fn recency(&self, command: &str) -> u64 {
        self.last_used.get(command).copied().unwrap_or(0)
    }

    /// Move the selection cursor by `delta` rows, clamping.
    pub fn step(&mut self, delta: i32) {
        let total = self.row_count();
        if total == 0 {
            self.selected = 0;
            self.first_visible = 0;
            return;
        }
        let next = (self.selected as i32 + delta).max(0).min(total as i32 - 1);
        self.selected = next as usize;
        self.ensure_selected_visible();
    }

    /// Total rows in the virtual list = math preview (1 row) + filtered
    /// commands.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.math_offset() + self.filtered.len()
    }

    /// First visible virtual row in the capped result window.
    #[must_use]
    pub(crate) fn first_visible(&self) -> usize {
        self.first_visible
    }

    /// Count of virtual rows visible at once.
    #[must_use]
    pub(crate) fn visible_row_count(&self) -> usize {
        self.row_count().min(PALETTE_VISIBLE_ROW_LIMIT)
    }

    /// Visible virtual row range.
    #[must_use]
    pub(crate) fn visible_row_range(&self) -> std::ops::Range<usize> {
        let start = self.first_visible.min(self.max_first_visible());
        start..(start + self.visible_row_count()).min(self.row_count())
    }

    /// Convert a visible list-row index to an absolute virtual row.
    #[must_use]
    pub(crate) fn virtual_row_for_visible(&self, visible_row_idx: usize) -> Option<usize> {
        let row = self.first_visible.checked_add(visible_row_idx)?;
        (row < self.row_count()).then_some(row)
    }

    /// Select the row at a visible list-row index.
    pub(crate) fn select_visible_row(&mut self, visible_row_idx: usize) -> bool {
        let Some(row) = self.virtual_row_for_visible(visible_row_idx) else {
            return false;
        };
        self.select_row(row)
    }

    /// Convert an absolute virtual row to an `all` index for a command row.
    #[must_use]
    pub(crate) fn command_index_for_row(&self, row: usize) -> Option<usize> {
        let command_row = row.checked_sub(self.math_offset())?;
        self.filtered.get(command_row).copied()
    }

    /// Scroll the capped result window by `delta_rows`.
    pub(crate) fn scroll_visible_rows(&mut self, delta_rows: i32) -> bool {
        let max_first_visible = self.max_first_visible();
        let next = if delta_rows < 0 {
            self.first_visible
                .saturating_sub(delta_rows.unsigned_abs() as usize)
        } else {
            self.first_visible.saturating_add(delta_rows as usize)
        }
        .min(max_first_visible);
        if next == self.first_visible {
            return false;
        }
        self.first_visible = next;
        true
    }

    fn select_row(&mut self, row: usize) -> bool {
        if row >= self.row_count() || self.selected == row {
            return false;
        }
        self.selected = row;
        self.ensure_selected_visible();
        true
    }

    fn ensure_selected_visible(&mut self) {
        let total = self.row_count();
        if total == 0 {
            self.first_visible = 0;
            return;
        }
        let visible = self.visible_row_count();
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + visible {
            self.first_visible = self.selected + 1 - visible;
        }
        self.clamp_first_visible();
    }

    fn clamp_first_visible(&mut self) {
        self.first_visible = self.first_visible.min(self.max_first_visible());
    }

    fn max_first_visible(&self) -> usize {
        self.row_count().saturating_sub(PALETTE_VISIBLE_ROW_LIMIT)
    }

    /// `1` when a math preview row is present, `0` otherwise.
    #[must_use]
    pub(crate) fn math_offset(&self) -> usize {
        usize::from(self.math_preview.is_some())
    }

    /// `true` when the math preview row is the currently-selected row.
    /// Implies [`Self::math_preview`] is `Some`.
    #[must_use]
    pub(crate) fn math_row_selected(&self) -> bool {
        self.math_preview.is_some() && self.selected == 0
    }

    /// Currently-selected command entry, if any. Returns `None` when the
    /// math preview row is selected.
    #[must_use]
    pub(crate) fn selected_entry(&self) -> Option<&PaletteEntry> {
        if self.math_row_selected() {
            return None;
        }
        let off = self.math_offset();
        let idx = self.selected.checked_sub(off)?;
        self.filtered.get(idx).and_then(|i| self.all.get(*i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, applicable: bool) -> PaletteEntry {
        PaletteEntry {
            command: name.into(),
            keybinding: None,
            description: None,
            applicable,
        }
    }

    #[test]
    fn empty_query_keeps_all_candidates() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("a.b", true), entry("c.d", true)]);
        assert_eq!(p.filtered.len(), 2);
    }

    #[test]
    fn refilter_drops_non_matches() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("editor.find", true), entry("view.zoom", true)]);
        p.input.set_text("zoom");
        p.refilter();
        assert_eq!(p.filtered.len(), 1);
        assert_eq!(p.selected_entry().unwrap().command, "view.zoom");
    }

    #[test]
    fn step_clamps_to_bounds() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("a", true), entry("b", true)]);
        p.step(-5);
        assert_eq!(p.selected, 0);
        p.step(50);
        assert_eq!(p.selected, 1);
    }

    #[test]
    fn grayed_entries_remain_in_list() {
        let mut p = Palette::new();
        p.set_candidates(vec![
            entry("editor.find", false),
            entry("editor.find_next", true),
        ]);
        assert_eq!(p.filtered.len(), 2);
        assert!(!p.all[p.filtered[0]].applicable || !p.all[p.filtered[1]].applicable);
    }

    #[test]
    fn math_intent_populates_preview_row() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("editor.find", true)]);
        p.input.set_text("5 + 3");
        p.refilter();
        let math = p.math_preview.as_ref().expect("math preview");
        assert_eq!(math.value, 8.0);
        assert_eq!(p.row_count(), 1 + p.filtered.len());
        assert!(p.math_row_selected());
        assert!(p.selected_entry().is_none());
    }

    #[test]
    fn command_query_does_not_populate_math() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("editor.find", true)]);
        p.input.set_text("find");
        p.refilter();
        assert!(p.math_preview.is_none());
        assert_eq!(p.row_count(), p.filtered.len());
        assert!(!p.math_row_selected());
    }

    #[test]
    fn step_walks_off_math_row_to_first_command() {
        // The math-intent grammar and the fuzzy-match grammar are
        // disjoint (math leads with a digit; commands lead with a
        // letter), so a single filter line can't be both. Force the
        // co-existence state directly to verify selection mapping.
        let mut p = Palette::new();
        p.set_candidates(vec![entry("editor.find", true)]);
        // Math row prepended; selected == 0 → math, selected == 1 → cmd.
        p.math_preview = Some(crate::palette_math::MathPreview {
            expr: "1+1".into(),
            value: 2.0,
        });
        assert_eq!(p.row_count(), 2);
        assert!(p.math_row_selected());
        assert!(p.selected_entry().is_none());
        p.step(1);
        assert!(!p.math_row_selected());
        assert_eq!(p.selected_entry().unwrap().command, "editor.find");
        p.step(-1);
        assert!(p.math_row_selected());
    }

    #[test]
    fn step_clamps_when_only_math_row_present() {
        let mut p = Palette::new();
        // No candidates registered.
        p.input.set_text("5 + 3");
        p.refilter();
        assert!(p.math_preview.is_some());
        assert_eq!(p.filtered.len(), 0);
        assert_eq!(p.row_count(), 1);
        p.step(5);
        assert_eq!(p.selected, 0);
        assert!(p.math_row_selected());
    }

    #[test]
    fn recently_used_command_ranks_above_alphabetical_peer() {
        let mut p = Palette::new();
        p.set_candidates(vec![
            entry("editor.alpha", true),
            entry("editor.bravo", true),
            entry("editor.charlie", true),
        ]);
        // Empty query → no curated defaults in this fixture, so the
        // alphabetical fallback puts `alpha` on top.
        p.refilter();
        assert_eq!(p.all[p.filtered[0]].command, "editor.alpha");
        // Use `editor.charlie`; on the next refilter it should jump
        // above its alphabetical peers.
        p.note_command_used("editor.charlie");
        p.refilter();
        assert_eq!(p.all[p.filtered[0]].command, "editor.charlie");
        // Use `editor.bravo`; it now beats charlie via recency tick.
        p.note_command_used("editor.bravo");
        p.refilter();
        assert_eq!(p.all[p.filtered[0]].command, "editor.bravo");
        assert_eq!(p.all[p.filtered[1]].command, "editor.charlie");
        assert_eq!(p.all[p.filtered[2]].command, "editor.alpha");
    }

    #[test]
    fn fuzzy_score_still_wins_over_recency() {
        let mut p = Palette::new();
        p.set_candidates(vec![
            entry("zzz.find", true),
            entry("editor.find_next", true),
        ]);
        // Make zzz.find recent; then type a query that scores
        // editor.find_next higher (contiguous match against "find_next").
        p.note_command_used("zzz.find");
        p.input.set_text("find_next");
        p.refilter();
        assert_eq!(p.all[p.filtered[0]].command, "editor.find_next");
    }

    #[test]
    fn unparseable_math_falls_back_to_command_filter() {
        // Leading digit triggers intent, but the trailing 'x' breaks the
        // parser → no math row, filter list is unaffected.
        let mut p = Palette::new();
        p.set_candidates(vec![entry("editor.find", true)]);
        p.input.set_text("5 + x");
        p.refilter();
        assert!(p.math_preview.is_none());
    }

    #[test]
    fn empty_query_uses_curated_default_before_alphabetical() {
        let mut p = Palette::new();
        p.set_candidates(vec![
            entry("view.toggle_minimap", true),
            entry("editor.alpha", true),
            entry("file.open", true),
        ]);
        assert_eq!(p.all[p.filtered[0]].command, "file.open");
        assert_eq!(p.all[p.filtered[1]].command, "view.toggle_minimap");
    }

    #[test]
    fn empty_query_recency_beats_curated_default() {
        let mut p = Palette::new();
        p.set_candidates(vec![
            entry("view.toggle_minimap", true),
            entry("file.open", true),
            entry("editor.alpha", true),
        ]);
        p.note_command_used("editor.alpha");
        p.refilter();
        assert_eq!(p.all[p.filtered[0]].command, "editor.alpha");
    }

    #[test]
    fn space_query_matches_underscore_command() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("view.pick_theme", true)]);
        p.input.set_text("pick theme");
        p.refilter();
        assert_eq!(p.selected_entry().unwrap().command, "view.pick_theme");
    }

    #[test]
    fn compact_query_matches_spaced_command_label() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("view.pick_theme", true)]);
        p.input.set_text("picktheme");
        p.refilter();
        assert_eq!(p.selected_entry().unwrap().command, "view.pick_theme");
    }

    #[test]
    fn token_prefix_query_finds_minimap() {
        let mut p = Palette::new();
        p.set_candidates(vec![
            entry("view.toggle_line_numbers", true),
            entry("view.toggle_minimap", true),
        ]);
        p.input.set_text("mini");
        p.refilter();
        assert_eq!(p.selected_entry().unwrap().command, "view.toggle_minimap");
    }

    #[test]
    fn alias_query_finds_settings() {
        let mut p = Palette::new();
        p.set_candidates(vec![entry("settings.open", true)]);
        p.input.set_text("preferences");
        p.refilter();
        assert_eq!(p.selected_entry().unwrap().command, "settings.open");
    }

    #[test]
    fn keyboard_step_keeps_selection_inside_visible_window() {
        let mut p = Palette::new();
        p.set_candidates(
            (0..12)
                .map(|i| entry(&format!("cmd.{i:02}"), true))
                .collect(),
        );
        p.step(11);
        assert_eq!(p.selected, 11);
        assert_eq!(p.first_visible(), 2);
        assert_eq!(p.visible_row_range(), 2..12);
    }

    #[test]
    fn wheel_scroll_clamps_to_available_rows() {
        let mut p = Palette::new();
        p.set_candidates(
            (0..20)
                .map(|i| entry(&format!("cmd.{i:02}"), true))
                .collect(),
        );
        assert!(p.scroll_visible_rows(4));
        assert_eq!(p.first_visible(), 4);
        assert!(p.scroll_visible_rows(99));
        assert_eq!(p.first_visible(), 10);
        assert!(p.scroll_visible_rows(-99));
        assert_eq!(p.first_visible(), 0);
    }
}
