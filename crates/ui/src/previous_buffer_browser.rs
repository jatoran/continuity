//! δ.4 — previous-buffer browser overlay state.
//!
//! Palette-mode list of every buffer that lives in the SQLite DB
//! (closed tabs included, trash excluded by default). Each row carries
//! the buffer id, a derived title, a humanized last-edited subtitle,
//! and a trashed flag. The window populates `all` from
//! [`continuity_persist::PersistClient::list_buffer_records`] each
//! time the overlay opens or the filter cycles.
//!
//! Thread ownership: UI thread of the owning [`crate::Window`]. The
//! state is created in `show_previous_buffer_browser_impl`, mutated by
//! the overlay step / confirm / cancel routes in
//! `window_overlays.rs`, and dropped by `Overlays::dismiss`.

use continuity_buffer::BufferId;
use continuity_persist::BufferListFilter;
use continuity_search::{score, FuzzyMatch};

use crate::text_input::TextInput;

/// One row in the previous-buffer browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousBufferRow {
    /// Underlying buffer id.
    pub id: BufferId,
    /// Resolved title (first non-empty line of latest snapshot, or
    /// `Untitled`).
    pub title: String,
    /// Humanized last-edited subtitle (e.g. `"2h ago · 12 edits"`).
    pub subtitle: String,
    /// `true` when the row's persist record carried a non-NULL
    /// `deleted_at`.
    pub is_trashed: bool,
}

/// Previous-buffer browser overlay state.
#[derive(Debug, Default)]
pub struct PreviousBufferBrowser {
    /// Search input (matched against the title).
    pub input: TextInput,
    /// Full candidate list, sorted by `last_touched DESC`.
    pub all: Vec<PreviousBufferRow>,
    /// Indices into `all` of currently-shown matches, in score order.
    pub filtered: Vec<usize>,
    /// Per-filtered-row fuzzy-match metadata (matched against the title).
    pub matches: Vec<FuzzyMatch>,
    /// Selected row within `filtered`.
    pub selected: usize,
    /// Which subset of buffers is currently listed.
    pub filter: BufferListFilter,
}

impl PreviousBufferBrowser {
    /// A fresh browser. Empty list; default filter is
    /// [`BufferListFilter::ActiveOnly`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the candidate list and re-run the fuzzy filter.
    pub fn set_candidates(&mut self, all: Vec<PreviousBufferRow>) {
        self.all = all;
        self.selected = 0;
        self.refilter();
    }

    /// Replace the active filter discriminant. The caller is expected
    /// to refresh `all` afterwards via [`Self::set_candidates`]; this
    /// method only updates the discriminant so the renderer's footer
    /// reflects the chord immediately.
    pub fn set_filter(&mut self, filter: BufferListFilter) {
        self.filter = filter;
    }

    /// Cycle `ActiveOnly → All → TrashedOnly → ActiveOnly`. Returns the
    /// new value so the caller can re-query persist.
    pub fn cycle_filter(&mut self) -> BufferListFilter {
        let next = match self.filter {
            BufferListFilter::ActiveOnly => BufferListFilter::All,
            BufferListFilter::All => BufferListFilter::TrashedOnly,
            BufferListFilter::TrashedOnly => BufferListFilter::ActiveOnly,
        };
        self.filter = next;
        next
    }

    /// Re-rank against the current query. Title score is primary; an
    /// empty query lists every entry in the original order.
    pub fn refilter(&mut self) {
        let q = self.input.text.as_str();
        let mut scored: Vec<(usize, FuzzyMatch)> = Vec::new();
        for (i, entry) in self.all.iter().enumerate() {
            if let Some(m) = score(q, &entry.title) {
                scored.push((i, m));
            } else if q.is_empty() {
                scored.push((
                    i,
                    FuzzyMatch {
                        score: 0,
                        matched_indices: Vec::new(),
                    },
                ));
            }
        }
        if !q.is_empty() {
            scored.sort_by(|a, b| {
                b.1.score
                    .cmp(&a.1.score)
                    .then_with(|| self.all[a.0].title.cmp(&self.all[b.0].title))
            });
        }
        self.filtered.clear();
        self.matches.clear();
        for (i, m) in scored {
            self.filtered.push(i);
            self.matches.push(m);
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    /// Move the selection cursor by `delta` rows, clamped to the
    /// filtered range.
    pub fn step(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as i32;
        let next = (self.selected as i32 + delta).max(0).min(len - 1);
        self.selected = next as usize;
    }

    /// Currently-selected entry, if any.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&PreviousBufferRow> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
    }

    /// Total candidate count.
    #[must_use]
    pub fn total(&self) -> usize {
        self.all.len()
    }

    /// Visible (filtered) row count.
    #[must_use]
    pub fn visible(&self) -> usize {
        self.filtered.len()
    }
}

/// Humanize a delta from `now_ms` back to `then_ms` ("just now",
/// `"3m ago"`, `"2h ago"`, `"4d ago"`, `"2026-01-04"`).
#[must_use]
pub fn humanize_age(now_ms: i64, then_ms: i64) -> String {
    let dt = now_ms.saturating_sub(then_ms).max(0);
    let secs = dt / 1_000;
    if secs < 45 {
        return "just now".into();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 14 {
        return format!("{days}d ago");
    }
    // Fall back to a coarse ISO date built from unix-ms without pulling
    // in `chrono`. Format: `YYYY-MM-DD` (UTC).
    let secs_total = then_ms / 1_000;
    let (year, month, day) = unix_secs_to_ymd(secs_total);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Compose a one-line subtitle from a humanized age and an edit count.
#[must_use]
pub fn compose_subtitle(age: &str, edit_count: u64, is_trashed: bool) -> String {
    let edits_label = if edit_count == 1 { "edit" } else { "edits" };
    if is_trashed {
        format!("[trashed] · {age} · {edit_count} {edits_label}")
    } else {
        format!("{age} · {edit_count} {edits_label}")
    }
}

/// Cheap unix-seconds → (year, month, day) UTC conversion via the
/// civil-from-days algorithm. Avoids pulling chrono just for the
/// fallback age label.
fn unix_secs_to_ymd(secs: i64) -> (i32, u32, u32) {
    let days = secs.div_euclid(86_400);
    // Howard Hinnant, "date" library — public-domain civil-from-days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str) -> PreviousBufferRow {
        PreviousBufferRow {
            id: BufferId::new(),
            title: title.into(),
            subtitle: String::new(),
            is_trashed: false,
        }
    }

    #[test]
    fn new_browser_is_empty() {
        let b = PreviousBufferBrowser::new();
        assert_eq!(b.total(), 0);
        assert_eq!(b.visible(), 0);
        assert!(b.selected_entry().is_none());
        assert_eq!(b.filter, BufferListFilter::ActiveOnly);
    }

    #[test]
    fn empty_query_lists_all() {
        let mut b = PreviousBufferBrowser::new();
        b.set_candidates(vec![row("alpha"), row("beta")]);
        assert_eq!(b.visible(), 2);
    }

    #[test]
    fn query_filters_by_title() {
        let mut b = PreviousBufferBrowser::new();
        b.set_candidates(vec![row("alpha"), row("beta"), row("gamma")]);
        b.input.set_text("be");
        b.refilter();
        assert_eq!(b.visible(), 1);
        assert_eq!(b.selected_entry().unwrap().title, "beta");
    }

    #[test]
    fn step_clamps_at_bounds() {
        let mut b = PreviousBufferBrowser::new();
        b.set_candidates(vec![row("a"), row("b"), row("c")]);
        b.step(-3);
        assert_eq!(b.selected, 0);
        b.step(50);
        assert_eq!(b.selected, 2);
    }

    #[test]
    fn cycle_filter_walks_three_values() {
        let mut b = PreviousBufferBrowser::new();
        assert_eq!(b.cycle_filter(), BufferListFilter::All);
        assert_eq!(b.cycle_filter(), BufferListFilter::TrashedOnly);
        assert_eq!(b.cycle_filter(), BufferListFilter::ActiveOnly);
    }

    #[test]
    fn humanize_age_thresholds() {
        let now = 10_000_000;
        assert_eq!(humanize_age(now, now - 10_000), "just now");
        assert_eq!(humanize_age(now, now - 90_000), "1m ago");
        assert_eq!(humanize_age(now, now - 60 * 60 * 1_000), "1h ago");
        assert_eq!(humanize_age(now, now - 5 * 24 * 60 * 60 * 1_000), "5d ago");
    }

    #[test]
    fn humanize_age_falls_back_to_iso_date_after_two_weeks() {
        let now = 1_700_000_000_000;
        let label = humanize_age(now, 1_600_000_000_000);
        assert!(label.starts_with("20"));
        assert_eq!(label.len(), 10);
    }

    #[test]
    fn compose_subtitle_pluralizes_edits() {
        assert_eq!(compose_subtitle("3m ago", 1, false), "3m ago · 1 edit");
        assert_eq!(compose_subtitle("3m ago", 5, false), "3m ago · 5 edits");
        assert_eq!(
            compose_subtitle("3m ago", 5, true),
            "[trashed] · 3m ago · 5 edits"
        );
    }

    #[test]
    fn unix_secs_matches_known_date() {
        // 2020-01-01T00:00:00Z = 1_577_836_800
        assert_eq!(unix_secs_to_ymd(1_577_836_800), (2020, 1, 1));
    }
}
