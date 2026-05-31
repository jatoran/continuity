//! δ.4 — `Window` wire-up for the previous-buffer browser overlay.
//!
//! Owns three flows:
//!  * `show_previous_buffer_browser_impl` — query persist for the
//!    [`BufferListFilter::ActiveOnly`] subset and install the overlay.
//!  * `cycle_previous_buffer_browser_filter` — chord handler that
//!    walks the filter discriminant and re-queries persist.
//!  * `confirm_previous_buffer_browser` — adopt the highlighted row
//!    as a fresh tab in the focused pane, recovering the buffer from
//!    the persist layer when it isn't already in editor state.
//!
//! Thread ownership: every method runs on the UI thread of one
//! [`Window`]. Persist queries block on a reply channel; the queries
//! are infrequent (only when the overlay opens or the filter cycles)
//! and bounded by the buffer count, so the block is acceptable.

use continuity_persist::{BufferListFilter, BufferRecord};

use crate::previous_buffer_browser::{compose_subtitle, humanize_age, PreviousBufferRow};
use crate::Window;

impl Window {
    /// δ.4 — show the previous-buffer browser overlay. Queries persist
    /// for the [`BufferListFilter::ActiveOnly`] subset (the headline
    /// case), builds humanized rows, and installs the palette-mode
    /// overlay. Silently no-ops when no persist client is configured
    /// (headless tests).
    pub(crate) fn show_previous_buffer_browser_impl(&mut self) -> Result<(), crate::Error> {
        let filter = BufferListFilter::ActiveOnly;
        let rows = self.query_previous_buffer_rows(filter);
        self.overlays.open_previous_buffer_browser(rows, filter);
        self.focus_overlay_input();
        self.request_repaint();
        Ok(())
    }

    /// δ.4 — chord handler bound to `Ctrl+T` while the overlay is
    /// open: walk the filter discriminant and re-query persist.
    pub(crate) fn cycle_previous_buffer_browser_filter(&mut self) {
        let Some(browser) = self.overlays.previous_buffer_browser_mut() else {
            return;
        };
        let next = browser.cycle_filter();
        let rows = self.query_previous_buffer_rows(next);
        if let Some(browser) = self.overlays.previous_buffer_browser_mut() {
            browser.set_candidates(rows);
        }
        self.request_repaint();
    }

    /// δ.4 — commit path. Adopts the highlighted buffer as a new tab
    /// in the focused pane, recovering it from persist when it isn't
    /// already in editor state.
    pub(crate) fn confirm_previous_buffer_browser(&mut self) {
        let Some(buffer_id) = self
            .overlays
            .previous_buffer_browser_mut()
            .and_then(|b| b.selected_entry().map(|e| e.id))
        else {
            self.dismiss_overlay_and_blur();
            return;
        };
        // If the buffer isn't in editor state, recover it from persist
        // and adopt before opening the tab. `editor.snapshot` returning
        // `Some` is the cheap "already loaded" check.
        if self.editor.snapshot(buffer_id).is_none() {
            self.recover_and_adopt_buffer(buffer_id);
        }
        self.adopt_buffer_as_new_tab(buffer_id);
        self.dismiss_overlay_and_blur();
        self.maybe_trigger_jump_glow(None);
    }

    /// Build the row list for `filter` by querying persist + humanizing.
    fn query_previous_buffer_rows(&self, filter: BufferListFilter) -> Vec<PreviousBufferRow> {
        let Some(client) = self.persist_client.as_ref() else {
            return Vec::new();
        };
        let records: Vec<BufferRecord> = client.list_buffer_records(filter).unwrap_or_default();
        let now = self.now_ms() as i64;
        records
            .into_iter()
            .map(|r| {
                let title = r.title.clone().unwrap_or_else(|| "Untitled".to_string());
                let age = humanize_age(now, r.last_touched_ms);
                let subtitle = compose_subtitle(&age, r.edit_count, r.is_trashed);
                PreviousBufferRow {
                    id: r.id,
                    title,
                    subtitle,
                    is_trashed: r.is_trashed,
                }
            })
            .collect()
    }

    /// δ.4 (Stage B) — open the time-machine slider against the
    /// highlighted row's buffer. Driven by the `Ctrl+R` chord while the
    /// browser overlay is open. Adopts the buffer if it isn't already
    /// in editor state, then hands off to the existing time-machine
    /// open path. Silently no-ops when no row is highlighted.
    pub(crate) fn open_timeline_for_highlighted_closed_buffer(&mut self) {
        let Some(buffer_id) = self
            .overlays
            .previous_buffer_browser()
            .and_then(|b| b.selected_entry().map(|e| e.id))
        else {
            return;
        };
        let _ = self.open_timeline_for_closed_buffer_impl(buffer_id);
    }

    /// δ.4 (Stage B) — adopt `buffer_id` if it isn't already in editor
    /// state, install it as a fresh tab, then open the existing
    /// time-machine slider on that buffer. Reuses
    /// [`Self::open_buffer_timeline_impl`] so the slider behavior is
    /// identical to the headline I1 chord.
    pub(crate) fn open_timeline_for_closed_buffer_impl(
        &mut self,
        buffer_id: continuity_buffer::BufferId,
    ) -> Result<(), crate::Error> {
        if self.editor.snapshot(buffer_id).is_none() {
            self.recover_and_adopt_buffer(buffer_id);
        }
        // Reuse the existing tab-adoption path so the buffer becomes
        // the active tab on the focused pane, then open the slider.
        self.adopt_buffer_as_new_tab(buffer_id);
        self.dismiss_overlay_and_blur();
        self.open_buffer_timeline_impl()
    }

    /// δ.4 — fetch the buffer's persisted snapshot + edit log, rebuild
    /// the rope, and adopt it into the editor. No-op when persist is
    /// unconfigured or the buffer has no snapshot.
    fn recover_and_adopt_buffer(&self, buffer_id: continuity_buffer::BufferId) {
        let Some(client) = self.persist_client.as_ref() else {
            return;
        };
        let Ok(Some(recovered)) = continuity_persist::recover_buffer(client, buffer_id) else {
            return;
        };
        let now = self.now_ms() as i64;
        let _ = self
            .editor
            .adopt_buffer(recovered.buffer, recovered.next_seq, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: `humanize_age` round-trips and `compose_subtitle`
    /// composes are unit-tested in `previous_buffer_browser.rs`. This
    /// module exists primarily as the Window-method host; integration
    /// coverage of the persist→overlay flow lives in
    /// `crates/ui/tests/previous_buffer_browser_e2e.rs`.
    #[test]
    fn module_compiles() {
        let _ = BufferListFilter::ActiveOnly;
    }
}
