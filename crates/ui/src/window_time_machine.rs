//! Time-machine slider state (I1), named-snapshot label staging (I1),
//! and the metrics buffer pipeline (I2): per-pane state, the
//! `show_metrics_buffer` / `purge_metrics` impls on `Window`, the
//! keystroke → WPM → `record_metrics_delta` tap, and the 1 Hz metrics
//! repaint cadence.
//!
//! Sibling of [`crate::window_pane_modes`]; pulled out of
//! `window_view_options` so the time-machine + metrics surface area lives
//! in one file.
//!
//! - **I1 timeline**: the overlay `visible` flag + preview revision are
//!   tracked here. The slider HUD render lands when the overlay-paint
//!   integration in `overlay_render` is wired (the palette-mode
//!   framework A1 is the host).
//! - **I1 named snapshots**: `pending_label` is set by
//!   `mark_next_snapshot`. The same call forwards the label to
//!   `EditorHandle::set_pending_snapshot_label`, which stores it in a
//!   per-buffer map on the editor thread. When the snapshot policy
//!   commits the next snapshot for that buffer (or shutdown flushes a
//!   final one), `core::dispatch` fires
//!   `PersistClient::set_snapshot_label(buffer_id, revision, Some(label))`
//!   on the just-written row and clears the staged label.
//! - **I2 metrics**: keystrokes feed [`Window::wpm_tracker`] and
//!   accumulate into [`Window::metrics_pending`]; once per second the
//!   delta is flushed to `PersistClient::record_metrics_delta`. The
//!   dedicated metrics buffer is opened (or re-focused) by
//!   [`Window::show_metrics_buffer_impl`] and the 1 Hz repaint timer
//!   is started while it is the active tab.
//!
//! Thread ownership: UI thread of one window — `Window` is the only
//! mutator.

use continuity_buffer::{BufferId, Revision};
use continuity_persist::MetricsDailyDelta;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::window::Window;
use crate::window_timers::{METRICS_REPAINT_TIMER_ID, METRICS_REPAINT_TIMER_MS};

/// Minimum wall-clock gap between successive `record_metrics_delta`
/// flushes. Matches the spec §I2 "1 Hz while active" cadence and keeps
/// SQLite UPSERT pressure bounded under sustained typing.
pub(crate) const METRICS_FLUSH_INTERVAL_MS: u64 = 1_000;

/// Maximum `active_ms` credit awarded per keystroke when computing the
/// gap to the previous keystroke. The §I2 "active" definition is
/// "foreground + recent input"; a typing pause longer than this is
/// treated as idle.
pub(crate) const METRICS_ACTIVE_GAP_CAP_MS: u64 = 2_000;

/// Why a keystroke was recorded into the metrics tap. Drives which
/// counters in [`MetricsDailyDelta`] are incremented.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MetricsKeystroke {
    /// A character was inserted at one or more selections.
    Inserted { chars: u64 },
    /// A character was removed at one or more selections.
    Deleted { chars: u64 },
}

/// Per-pane Phase I state.
#[derive(Debug, Clone, Default)]
pub struct TimeMachineState {
    /// §I1: the time-machine slider overlay is currently visible.
    pub timeline_visible: bool,
    /// §I1: the revision the slider is currently previewing. `None`
    /// means "at head".
    pub timeline_preview_revision: Option<Revision>,
    /// §I1: label staged for the next snapshot the editor commits
    /// against the active buffer. `None` once the snapshot lands and
    /// the persist call has been issued.
    pub pending_snapshot_label: Option<String>,
    /// §I2: id of the metrics buffer once it has been opened in this
    /// window. Reused on subsequent `view.metrics` chords.
    pub metrics_buffer_id: Option<BufferId>,
    /// §I2: 1 Hz repaint pacing token — set when the metrics buffer
    /// becomes the active tab; cleared on tab change.
    pub metrics_repaint_due: bool,
}

impl Window {
    /// §I1: open the timeline overlay for the focused pane.
    ///
    /// Scaffolding — the overlay paint lands when palette-mode A1's
    /// host paints the slider. This impl flips the visible flag and
    /// stages the preview revision at the active buffer's head so the
    /// first read of `timeline_preview_revision` is well-defined.
    pub(crate) fn open_buffer_timeline_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.time_machine.timeline_visible = true;
        self.view_options.time_machine.timeline_preview_revision = None;
        self.request_repaint();
        Ok(())
    }

    /// §I1: stamp `label` onto the next snapshot the editor commits.
    /// Empty `label` clears any pending staged label (treated as
    /// "never mind").
    ///
    /// Mirrors the staged value in [`TimeMachineState::pending_snapshot_label`]
    /// (so the HUD / status chips can display "label pending") and
    /// forwards to [`continuity_core::EditorHandle::set_pending_snapshot_label`]
    /// so the snapshot-policy callback in `core::dispatch` can stamp
    /// the label on the next committed snapshot row for this buffer.
    pub(crate) fn mark_next_snapshot_impl(&mut self, label: &str) -> Result<(), crate::Error> {
        let staged = if label.is_empty() {
            None
        } else {
            Some(label.to_owned())
        };
        self.view_options.time_machine.pending_snapshot_label = staged.clone();
        self.editor
            .set_pending_snapshot_label(self.buffer_id, staged);
        Ok(())
    }

    /// §I2: open (or focus) the dedicated metrics buffer in this
    /// window.
    ///
    /// First call allocates a synthetic empty buffer (no on-disk
    /// backing), pins its id in [`crate::window_view_options::ViewOptions::time_machine`],
    /// and opens it as a tab in the focused pane. Subsequent calls
    /// switch focus back to the existing tab. Either way the 1 Hz
    /// repaint timer is armed.
    pub(crate) fn show_metrics_buffer_impl(&mut self) -> Result<(), crate::Error> {
        let existing = self.view_options.time_machine.metrics_buffer_id;
        let target = if let Some(id) = existing {
            // The synthetic buffer was previously opened — find its tab
            // and refocus, or re-adopt it as a new tab if the user has
            // since closed it.
            if self.focus_existing_tab_for(id) {
                id
            } else {
                self.adopt_buffer_as_new_tab(id);
                id
            }
        } else {
            // First call: ask the editor to allocate an empty buffer
            // for us and adopt it as the active tab. Its rope is never
            // mutated by user keystrokes — the renderer dispatches on
            // the matching id and paints the metrics panel instead.
            let id = self.editor.open_buffer(String::new());
            self.view_options.time_machine.metrics_buffer_id = Some(id);
            self.adopt_buffer_as_new_tab(id);
            id
        };
        let _ = target;
        self.view_options.time_machine.metrics_repaint_due = true;
        self.start_metrics_repaint_timer();
        self.request_repaint();
        Ok(())
    }

    /// §I2: drop every row from `metrics_daily` via the persist client.
    /// No-op when the window was built without a persist client (test
    /// harnesses).
    pub(crate) fn purge_metrics_impl(&mut self) -> Result<(), crate::Error> {
        if let Some(client) = self.persist_client.as_ref() {
            if let Err(e) = client.purge_metrics() {
                eprintln!("metrics.purge: persist call failed: {e}");
            }
        }
        // Drop in-flight pending so the next flush doesn't immediately
        // re-create today's row.
        self.metrics_pending = MetricsDailyDelta::default();
        self.metrics_last_flush_ms = self.now_ms();
        self.view_options.time_machine.metrics_repaint_due = true;
        self.request_repaint();
        Ok(())
    }

    /// §I2: record one keystroke into the metrics tap. Always called
    /// from the UI thread — `Window` is single-writer for both the
    /// `WpmTracker` and the pending delta.
    pub(crate) fn note_metrics_keystroke(&mut self, kind: MetricsKeystroke) {
        let now_ms = self.now_ms();
        if self.metrics_last_flush_ms == 0 {
            self.metrics_last_flush_ms = now_ms;
        }
        // Active-time accounting: credit the gap since the previous
        // keystroke, capped so an idle pause doesn't count.
        if self.metrics_last_keystroke_ms != 0 {
            let gap = now_ms.saturating_sub(self.metrics_last_keystroke_ms);
            self.metrics_pending.active_ms = self
                .metrics_pending
                .active_ms
                .saturating_add(gap.min(METRICS_ACTIVE_GAP_CAP_MS));
        }
        self.metrics_last_keystroke_ms = now_ms;
        self.metrics_pending.keystrokes = self.metrics_pending.keystrokes.saturating_add(1);
        match kind {
            MetricsKeystroke::Inserted { chars } => {
                self.metrics_pending.chars_typed =
                    self.metrics_pending.chars_typed.saturating_add(chars);
                // One sample per inserted character keeps the rolling
                // 60 s WPM responsive without flooding the tracker.
                for _ in 0..chars {
                    self.wpm_tracker.record(now_ms);
                }
            }
            MetricsKeystroke::Deleted { chars } => {
                self.metrics_pending.chars_deleted =
                    self.metrics_pending.chars_deleted.saturating_add(chars);
            }
        }
        self.flush_metrics_if_due(now_ms);
    }

    /// §I2: flush the pending [`MetricsDailyDelta`] to the persist
    /// thread when at least [`METRICS_FLUSH_INTERVAL_MS`] has elapsed
    /// since the last flush. No-op when there is no persist client or
    /// no observed keystrokes since the last successful flush.
    pub(crate) fn flush_metrics_if_due(&mut self, now_ms: u64) {
        if self.persist_client.is_none() {
            return;
        }
        if self.metrics_pending.keystrokes == 0 {
            // Nothing observed since the last flush — keep the row
            // quiescent rather than touching `updated_at_ms`.
            return;
        }
        let last = self.metrics_last_flush_ms;
        if last != 0 && now_ms.saturating_sub(last) < METRICS_FLUSH_INTERVAL_MS {
            return;
        }
        self.flush_metrics_now(now_ms);
    }

    /// §I2: send the pending delta to the persist thread immediately
    /// and reset the in-flight accumulator. Called from the 1 Hz
    /// fall-through in [`Self::flush_metrics_if_due`] and from the
    /// metrics-buffer repaint timer.
    pub(crate) fn flush_metrics_now(&mut self, now_ms: u64) {
        let Some(client) = self.persist_client.as_ref() else {
            return;
        };
        if self.metrics_pending.keystrokes == 0 {
            self.metrics_last_flush_ms = now_ms;
            return;
        }
        let wpm_sample = Some(self.wpm_tracker.wpm_now(now_ms));
        let mut delta = std::mem::take(&mut self.metrics_pending);
        delta.day_iso = crate::window_metrics_paint::day_iso_from_unix_ms(now_ms);
        delta.wpm_sample = wpm_sample;
        delta.now_ms = i64::try_from(now_ms).unwrap_or(i64::MAX);
        if let Err(e) = client.record_metrics_delta(delta) {
            eprintln!("record_metrics_delta failed: {e}");
        }
        self.metrics_last_flush_ms = now_ms;
    }

    /// §I2: arm the 1 Hz metrics-buffer repaint timer. Idempotent.
    pub(crate) fn start_metrics_repaint_timer(&mut self) {
        if self.metrics_repaint_active {
            return;
        }
        if self.hwnd.is_invalid() {
            return;
        }
        let id = unsafe {
            SetTimer(
                Some(self.hwnd),
                METRICS_REPAINT_TIMER_ID,
                METRICS_REPAINT_TIMER_MS,
                None,
            )
        };
        if id != 0 {
            self.metrics_repaint_active = true;
        }
    }

    /// §I2: tear down the 1 Hz metrics-buffer repaint timer. Idempotent.
    pub(crate) fn stop_metrics_repaint_timer(&mut self) {
        if !self.metrics_repaint_active {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), METRICS_REPAINT_TIMER_ID);
        }
        self.metrics_repaint_active = false;
    }

    /// §I2: WM_TIMER tick at 1 Hz while the metrics buffer is the
    /// active tab. Flushes any pending delta so the heatmap reflects
    /// up-to-the-second activity, then invalidates the client area.
    pub(crate) fn on_metrics_repaint_tick(&mut self, hwnd: windows::Win32::Foundation::HWND) {
        if !self.is_metrics_buffer_active() {
            self.stop_metrics_repaint_timer();
            return;
        }
        let now_ms = self.now_ms();
        self.flush_metrics_now(now_ms);
        self.view_options.time_machine.metrics_repaint_due = true;
        self.invalidate(hwnd);
    }

    /// §I2: `true` when the focused pane's active tab is the dedicated
    /// metrics buffer.
    pub(crate) fn is_metrics_buffer_active(&self) -> bool {
        match self.view_options.time_machine.metrics_buffer_id {
            Some(id) => self.buffer_id == id,
            None => false,
        }
    }

    /// §I2: scan every group for a tab whose buffer id is `target`. If
    /// found, focus that group, activate the tab, and refresh focused-
    /// pane scalars (mirrors [`Window::adopt_buffer_as_new_tab`]).
    /// Returns `true` when an existing tab was activated.
    pub(crate) fn focus_existing_tab_for(&mut self, target: BufferId) -> bool {
        let mut found: Option<(crate::pane_tree::PaneId, crate::pane_tree::TabId)> = None;
        for (pane_id, group) in &self.tree.groups {
            for tab_id in &group.tabs {
                if let Some(tab) = self.tree.tabs.get(tab_id) {
                    if tab.buffer_id == target {
                        found = Some((*pane_id, *tab_id));
                        break;
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        let Some((pane_id, tab_id)) = found else {
            return false;
        };
        self.save_current_right_edge_chrome_state();
        self.tree.focus(pane_id);
        if let Some(group) = self.tree.groups.get_mut(&pane_id) {
            group.activate(tab_id);
        }
        self.apply_new_pane_state(target);
        self.refresh_focused_viewport();
        self.refresh_language();
        self.maybe_submit_decoration();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_time_machine_state_is_quiescent() {
        let s = TimeMachineState::default();
        assert!(!s.timeline_visible);
        assert!(s.timeline_preview_revision.is_none());
        assert!(s.pending_snapshot_label.is_none());
        assert!(s.metrics_buffer_id.is_none());
        assert!(!s.metrics_repaint_due);
    }

    #[test]
    fn pending_snapshot_label_round_trips() {
        let s = TimeMachineState {
            pending_snapshot_label: Some("pre-refactor".into()),
            ..Default::default()
        };
        assert_eq!(s.pending_snapshot_label.as_deref(), Some("pre-refactor"));
    }

    #[test]
    fn timeline_preview_revision_stages_a_value() {
        let s = TimeMachineState {
            timeline_preview_revision: Some(Revision(7)),
            ..Default::default()
        };
        assert_eq!(s.timeline_preview_revision, Some(Revision(7)));
    }

    /// §I2 focus/blur lifecycle for the 1 Hz repaint timer, exercised
    /// against a pure state-machine simulation of `Window`'s
    /// `metrics_repaint_active` flag + the `on_metrics_repaint_tick`
    /// self-disarm path. We can't fire a real `SetTimer` /
    /// `KillTimer` without the Win32 harness, but the spec invariants
    /// (arm on focus, disarm on blur, self-disarm when the timer
    /// fires while the metrics buffer is no longer the active tab)
    /// are observable on the flag.
    mod focus_blur_timer {
        /// Model of just enough `Window` state for the timer
        /// transition rules — `metrics_repaint_active` is the only
        /// bit Win32 reads through `SetTimer`/`KillTimer` calls and
        /// the only bit the spec language pins (§I2 line 3139-3140:
        /// "throttled to 1 Hz while active, paused when not
        /// visible"). The real `Window` impl flips this same bit and
        /// then calls `SetTimer`/`KillTimer`; here we exercise the
        /// flip rules.
        #[derive(Default, Debug)]
        struct FakeRepaintTimer {
            metrics_repaint_active: bool,
            metrics_buffer_active: bool,
            window_active: bool,
        }

        impl FakeRepaintTimer {
            /// Mirrors `Window::start_metrics_repaint_timer`: arm the
            /// timer when the metrics buffer becomes the focused tab
            /// (idempotent).
            fn arm(&mut self) {
                if self.metrics_repaint_active {
                    return;
                }
                if !self.window_active {
                    return;
                }
                self.metrics_repaint_active = true;
            }

            /// Mirrors `Window::stop_metrics_repaint_timer`: disarm
            /// the timer on tab switch / focus loss / window
            /// deactivation (idempotent).
            fn disarm(&mut self) {
                if !self.metrics_repaint_active {
                    return;
                }
                self.metrics_repaint_active = false;
            }

            /// Mirrors `Window::on_metrics_repaint_tick`: when the
            /// timer fires, self-disarm if the metrics buffer is no
            /// longer the active tab. Returns whether the repaint
            /// should proceed.
            fn tick(&mut self) -> bool {
                if !self.metrics_buffer_active || !self.window_active {
                    self.disarm();
                    return false;
                }
                true
            }
        }

        #[test]
        fn arm_is_idempotent_when_metrics_buffer_active() {
            let mut t = FakeRepaintTimer {
                window_active: true,
                metrics_buffer_active: true,
                ..Default::default()
            };
            t.arm();
            assert!(t.metrics_repaint_active);
            t.arm();
            assert!(t.metrics_repaint_active);
        }

        #[test]
        fn arm_is_noop_when_window_inactive() {
            let mut t = FakeRepaintTimer {
                metrics_buffer_active: true,
                window_active: false,
                ..Default::default()
            };
            t.arm();
            assert!(!t.metrics_repaint_active);
        }

        #[test]
        fn disarm_is_idempotent() {
            let mut t = FakeRepaintTimer {
                metrics_repaint_active: true,
                ..Default::default()
            };
            t.disarm();
            assert!(!t.metrics_repaint_active);
            // Second call is a no-op.
            t.disarm();
            assert!(!t.metrics_repaint_active);
        }

        #[test]
        fn tick_self_disarms_when_tab_switched_away() {
            let mut t = FakeRepaintTimer {
                metrics_repaint_active: true,
                metrics_buffer_active: false,
                window_active: true,
            };
            assert!(!t.tick());
            assert!(
                !t.metrics_repaint_active,
                "tick must disarm when the metrics buffer is no longer focused"
            );
        }

        #[test]
        fn tick_self_disarms_when_window_deactivates() {
            let mut t = FakeRepaintTimer {
                metrics_repaint_active: true,
                metrics_buffer_active: true,
                window_active: false,
            };
            assert!(!t.tick());
            assert!(
                !t.metrics_repaint_active,
                "tick must disarm when the window loses activation"
            );
        }

        #[test]
        fn tick_continues_when_still_active() {
            let mut t = FakeRepaintTimer {
                metrics_repaint_active: true,
                metrics_buffer_active: true,
                window_active: true,
            };
            assert!(t.tick());
            assert!(t.metrics_repaint_active);
        }

        #[test]
        fn full_focus_blur_lifecycle() {
            let mut t = FakeRepaintTimer {
                window_active: true,
                ..Default::default()
            };
            // 1. User chords view.metrics → buffer becomes active.
            t.metrics_buffer_active = true;
            t.arm();
            assert!(t.metrics_repaint_active);
            // 2. Tick keeps the timer running.
            assert!(t.tick());
            assert!(t.metrics_repaint_active);
            // 3. User switches tab → tick self-disarms.
            t.metrics_buffer_active = false;
            assert!(!t.tick());
            assert!(!t.metrics_repaint_active);
            // 4. User chords view.metrics again → re-arms.
            t.metrics_buffer_active = true;
            t.arm();
            assert!(t.metrics_repaint_active);
            // 5. Window deactivates → tick self-disarms.
            t.window_active = false;
            assert!(!t.tick());
            assert!(!t.metrics_repaint_active);
        }
    }
}
