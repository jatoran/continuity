//! `Window::dispatch_selection_edit` — the single entry point for routing
//! every selection-aware edit through the core thread.
//!
//! Extracted from `selection.rs` to keep that file under the 600-line
//! conventions cap once α.1 added the edit-region pulse hook and the
//! persist-queue motion-timer arm.

use continuity_core::SelectionEdit;

use crate::edit_trace;
use crate::paint_trace::{is_trace_enabled, log_event, EventScope};
use crate::Window;

impl Window {
    /// Apply `edit` through the editor handle, then update UI-thread
    /// state that depends on the edit landing: the δ.1 last-edit-cursor
    /// stack, the α.1 edit-region pulse (for structural edits only), and
    /// the α.1 persist-queue motion-timer arm.
    pub(crate) fn dispatch_selection_edit(
        &mut self,
        edit: SelectionEdit,
    ) -> Result<(), continuity_command::Error> {
        self.cancel_scroll_inertia();
        // δ.1 — record the pre-edit primary-caret position so
        // `editor.goto_last_edit` can jump back to it. Captured BEFORE
        // the apply so it points at where the edit happened, not where
        // the caret landed afterward. α.1 reuses the same pre-snapshot
        // for the edit-region pulse range computation.
        let pre = self.editor.snapshot(self.buffer_id);
        let pre_edit_caret = pre
            .as_ref()
            .and_then(|s| s.selections().first().map(|sel| sel.head));
        let pre_line_count = pre.as_ref().map(|s| s.rope_snapshot().rope().len_lines());
        let should_pulse = crate::edit_pulse::is_structural_edit(&edit);
        // ε.7 — bracket the core round-trip with an `EventScope` so
        // `event:edit_apply` reports the UI-thread block on
        // `EditorHandle::apply_selection_edit`. `kind` is captured
        // BEFORE `edit` moves into the core message; `detail_of`
        // only allocates when tracing is on.
        let kind = edit_trace::kind_of(&edit);
        let edit_seq = is_trace_enabled().then(crate::paint_trace::next_edit_seq);
        let _edit_seq_guard = edit_seq.map(crate::paint_trace::bind_edit_seq);
        let _scope = is_trace_enabled().then(|| {
            EventScope::with_detail(
                "edit_apply",
                format!(
                    "kind={kind} entry=dispatch_selection_edit {}",
                    edit_trace::detail_of(&edit)
                ),
            )
        });
        // Read the input-burst counter BEFORE the apply so the
        // post-paint coalescing gate uses the count from this paint
        // cycle, not including the edit we're about to land.
        let is_first_edit_since_paint = crate::paint_trace::edits_since_paint() == 0;
        let result = {
            let _s = is_trace_enabled().then(|| EventScope::new("edit_core_roundtrip"));
            self.editor
                .apply_selection_edit_with_seq(self.buffer_id, edit, edit_seq)
        };
        if matches!(&result, Ok(Some(_))) {
            crate::paint_trace::note_edit_applied();
        }
        if is_trace_enabled() {
            log_event(
                "edit_apply_result",
                &edit_trace::format_result(kind, &result),
            );
        }
        result?;
        self.cancel_active_display_prewarm();
        // ε.5e + early-dispatch coalescing: give the projection
        // worker a head start on the new revision before the next
        // WM_PAINT, but only for the *first* edit in a paint cycle.
        // Subsequent edits in a burst would only redo the same
        // synchronous input-gathering on the UI thread (snapshot,
        // rope_deltas_since, fold/heading/reservation computation,
        // classify, build_request) — work the trace at 2026-05-17
        // showed costs ~43 ms per keystroke on a 6 k-line buffer
        // and starves WM_PAINT during held-key bursts. The worker's
        // latest-wins channel means missed early-dispatches just
        // make the next paint compute inline via Splice/Dirty.
        self.maybe_dispatch_projection_worker_early(is_first_edit_since_paint, "selection_edit");
        if let Some(pos) = pre_edit_caret {
            self.push_last_edit_position(pos);
        }
        if should_pulse {
            if let (Some(pre_head), Some(pre_lines)) = (pre_edit_caret, pre_line_count) {
                self.pulse_edit_region_after_dispatch(pre_head.line, pre_lines);
            }
        }
        // α.1 — every edit also makes the persistence queue grow.
        // Arm the motion timer so the persist-queue chip can fade in
        // and back out without waiting for the next keystroke's paint.
        // `start_motion_timer` is a no-op when the timer is already
        // running, and `has_active_motion` will stop it once the queue
        // drains.
        if self
            .persist_client
            .as_ref()
            .is_some_and(|c| c.unflushed_bytes() > 0)
        {
            self.start_motion_timer();
        }
        Ok(())
    }
}
