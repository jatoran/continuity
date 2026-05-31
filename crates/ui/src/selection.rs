//! Selection manipulation for [`Window`].
//!
//! Topic submodules:
//! - [`last_edit`] — δ.1 last-edit jump stack
//! - [`motions`] — horizontal / word / line / document-bounds motions
//! - [`vertical_motion`] — Up/Down with sticky intended-column and
//!   soft-wrap-aware [`FrameDisplay`](continuity_render::FrameDisplay)
//!   reuse. **Caret screen-y anchoring lives here** — see CLAUDE.md
//!   principles, do not reorder the rounding chain.
//! - [`multi_cursor`] — column / add-cursor / match-based commands
//! - [`region_select`] — word/line/paragraph/all + smart expand

mod last_edit;
mod motions;
mod multi_cursor;
mod region_select;
mod vertical_motion;

use continuity_core::SelectionEdit;
use continuity_text::{Position, Selection, SelectionKind};
use ropey::Rope;

use crate::edit_trace;
use crate::paint_trace::{is_trace_enabled, log_event, EventScope};
use crate::Window;

impl Window {
    pub(crate) fn insert_text_at_selections(
        &mut self,
        text: &str,
    ) -> Result<(), continuity_command::Error> {
        self.cancel_scroll_inertia();
        let edit = SelectionEdit::InsertText(text.to_string());
        let kind = edit_trace::kind_of(&edit);
        let edit_seq = is_trace_enabled().then(crate::paint_trace::next_edit_seq);
        let _edit_seq_guard = edit_seq.map(crate::paint_trace::bind_edit_seq);
        // ε.7 — same `edit_apply` scope shape `dispatch_selection_edit`
        // uses, so a typing-burst trace shows one bracket per keystroke
        // regardless of which funnel produced it. `entry=` names the
        // call site so the two paths can be distinguished without
        // changing the kind label.
        let _scope = is_trace_enabled().then(|| {
            EventScope::with_detail(
                "edit_apply",
                format!(
                    "kind={kind} entry=insert_text_at_selections {}",
                    edit_trace::detail_of(&edit)
                ),
            )
        });
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
        self.maybe_dispatch_projection_worker_early(is_first_edit_since_paint, "insert_text");
        Ok(())
    }

    pub(crate) fn delete_back_at_selections(&mut self) -> Result<(), continuity_command::Error> {
        self.cancel_scroll_inertia();
        let edit = SelectionEdit::DeleteBack;
        let kind = edit_trace::kind_of(&edit);
        let edit_seq = is_trace_enabled().then(crate::paint_trace::next_edit_seq);
        let _edit_seq_guard = edit_seq.map(crate::paint_trace::bind_edit_seq);
        let _scope = is_trace_enabled().then(|| {
            EventScope::with_detail(
                "edit_apply",
                format!("kind={kind} entry=delete_back_at_selections"),
            )
        });
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
        self.maybe_dispatch_projection_worker_early(is_first_edit_since_paint, "delete_back");
        Ok(())
    }

    pub(crate) fn delete_forward_at_selections(&mut self) -> Result<(), continuity_command::Error> {
        self.cancel_scroll_inertia();
        let edit = SelectionEdit::DeleteForward;
        let kind = edit_trace::kind_of(&edit);
        let edit_seq = is_trace_enabled().then(crate::paint_trace::next_edit_seq);
        let _edit_seq_guard = edit_seq.map(crate::paint_trace::bind_edit_seq);
        let _scope = is_trace_enabled().then(|| {
            EventScope::with_detail(
                "edit_apply",
                format!("kind={kind} entry=delete_forward_at_selections"),
            )
        });
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
        self.maybe_dispatch_projection_worker_early(is_first_edit_since_paint, "delete_forward");
        Ok(())
    }

    // `dispatch_selection_edit` lives in `selection_dispatch.rs` to keep
    // `selection.rs` under the 600-line conventions cap. α.1 hung extra
    // logic on the dispatch path (edit-region pulse + motion-timer arm)
    // so the split happens here.

    // G5 selection arithmetic — implementation in `selection_arithmetic.rs`.

    pub(crate) fn map_selections<F>(&mut self, f: F) -> bool
    where
        F: FnOnce(&Rope, &[Selection]) -> Vec<Selection>,
    {
        let Some(snapshot) = self.current_snapshot() else {
            return false;
        };
        let next = f(snapshot.rope_snapshot().rope(), snapshot.selections());
        let changed = next.as_slice() != snapshot.selections();
        let count = next.len();
        let _scope = is_trace_enabled().then(|| {
            EventScope::with_detail(
                "selection_set",
                format!("entry=map_selections changed={changed} selections={count}"),
            )
        });
        let result = self.editor.set_selections(self.buffer_id, next);
        let ok = result.is_ok();
        if is_trace_enabled() {
            log_event(
                "selection_update",
                &format!("entry=map_selections changed={changed} selections={count} ok={ok}"),
            );
        }
        if ok {
            // Force caret visible + invalidate. Most keyboard paths
            // also invalidate via the WM_KEYDOWN arm in
            // `window_dispatch`, but multi-cursor commands routed via
            // the palette or other surfaces don't always; do it here
            // so adding a cursor never waits for the next keystroke to
            // become visible. `note_input_now` also flips
            // `caret_blink_visible = true` so a new cursor doesn't get
            // stuck in a blink-off frame.
            self.note_input_now();
            self.invalidate(self.hwnd);
        }
        ok
    }
}

pub(crate) fn match_selection(rope: &Rope, start: usize, len: usize) -> Selection {
    let anchor = Position::from_byte_offset(rope, start).unwrap_or(Position::ZERO);
    let head = Position::from_byte_offset(rope, start + len).unwrap_or(anchor);
    Selection::new(anchor, head, SelectionKind::Caret)
}

pub(crate) fn dedupe(selections: Vec<Selection>) -> Vec<Selection> {
    let mut out = Vec::new();
    for selection in selections {
        if !out.contains(&selection) {
            out.push(selection);
        }
    }
    out
}
