//! §H6 — `Ctrl+Tab` positional tab-switcher palette-mode state.
//!
//! Holds the snapshot of positional tab rows captured at overlay-open
//! time, the previewing cursor, and the id of the tab that was active
//! when the overlay opened (the revert target on Esc).
//!
//! Thread ownership: UI thread of the owning [`crate::Window`]. The
//! state is created by `show_tab_overlay_impl`, mutated by the overlay
//! step / confirm / cancel routes in `window_overlays.rs`, and dropped
//! by `Overlays::dismiss`.

use continuity_buffer::BufferId;

use crate::pane_tree::TabId;

/// One row in the tab-switcher overlay.
#[derive(Clone, Debug)]
pub struct TabSwitcherRow {
    /// Tab id this row represents.
    pub tab_id: TabId,
    /// Buffer the row previews.
    pub buffer_id: BufferId,
    /// Resolved tab title (label override → first non-empty line → `Untitled`).
    pub title: String,
    /// Optional path-style subtitle (Phase 15+ wires real paths; today
    /// the subtitle stays empty for in-memory buffers).
    pub subtitle: String,
    /// `true` when the underlying buffer has unsaved revisions at
    /// open time.
    pub dirty: bool,
}

/// Tab-switcher overlay state. Mirrors `QuickOpen` / `GotoHeading` in
/// shape: a fixed candidate list (no fuzzy filter — tabs are scarce
/// enough that an unfiltered positional list is the right UX) plus a
/// selection cursor.
#[derive(Debug, Default)]
pub struct TabSwitcher {
    /// All rows in positional order, captured at open time.
    pub rows: Vec<TabSwitcherRow>,
    /// Selection cursor into `rows` (the row currently previewed).
    pub selected: usize,
    /// Tab id active when the overlay opened — reverted to on Esc.
    pub original_active: Option<TabId>,
}

impl TabSwitcher {
    /// Build a new switcher with `rows` (positional order) and the
    /// id of the tab that was active when the overlay opened. The
    /// selection cursor starts on the next positional tab so the
    /// first overlay tick already previews "the tab you'd land on
    /// if you released Ctrl now."
    #[must_use]
    pub fn new(rows: Vec<TabSwitcherRow>, original_active: TabId, initial_delta: i32) -> Self {
        let original_idx = rows
            .iter()
            .position(|r| r.tab_id == original_active)
            .unwrap_or(0);
        let selected = if rows.is_empty() {
            0
        } else {
            let len = rows.len() as i32;
            ((original_idx as i32 + initial_delta).rem_euclid(len)) as usize
        };
        Self {
            rows,
            selected,
            original_active: Some(original_active),
        }
    }

    /// Move the selection cursor by `delta` rows, wrapping.
    pub fn step(&mut self, delta: i32) {
        if self.rows.len() < 2 {
            return;
        }
        let len = self.rows.len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    /// Currently-highlighted row, if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<&TabSwitcherRow> {
        self.rows.get(self.selected)
    }

    /// `true` when there is nothing to switch to.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(tab: TabId, title: &str) -> TabSwitcherRow {
        TabSwitcherRow {
            tab_id: tab,
            buffer_id: BufferId::new(),
            title: title.into(),
            subtitle: String::new(),
            dirty: false,
        }
    }

    #[test]
    fn initial_cursor_starts_on_next_positional_tab() {
        let a = TabId::fresh();
        let b = TabId::fresh();
        let c = TabId::fresh();
        let rows = vec![row(a, "a"), row(b, "b"), row(c, "c")];
        // Active = a, delta = +1 → cursor on b.
        let ts = TabSwitcher::new(rows.clone(), a, 1);
        assert_eq!(ts.selected, 1);
        assert_eq!(ts.selected_row().unwrap().tab_id, b);
        // Active = a, delta = -1 → cursor on c (wraps).
        let ts = TabSwitcher::new(rows, a, -1);
        assert_eq!(ts.selected, 2);
        assert_eq!(ts.selected_row().unwrap().tab_id, c);
    }

    #[test]
    fn step_wraps_in_both_directions() {
        let a = TabId::fresh();
        let b = TabId::fresh();
        let c = TabId::fresh();
        let rows = vec![row(a, "a"), row(b, "b"), row(c, "c")];
        let mut ts = TabSwitcher::new(rows, a, 1);
        assert_eq!(ts.selected, 1);
        ts.step(1);
        assert_eq!(ts.selected, 2);
        ts.step(1);
        assert_eq!(ts.selected, 0);
        ts.step(-1);
        assert_eq!(ts.selected, 2);
    }

    #[test]
    fn step_noop_with_one_row() {
        let a = TabId::fresh();
        let mut ts = TabSwitcher::new(vec![row(a, "a")], a, 1);
        ts.step(1);
        ts.step(-1);
        assert_eq!(ts.selected, 0);
    }

    #[test]
    fn empty_switcher_is_empty() {
        let ts = TabSwitcher::default();
        assert!(ts.is_empty());
        assert!(ts.selected_row().is_none());
    }
}
