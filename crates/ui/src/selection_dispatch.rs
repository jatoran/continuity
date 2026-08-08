//! `Window::dispatch_selection_edit` — the single entry point for routing
//! every selection-aware edit through the core thread.
//!
//! Extracted from `selection.rs` to keep that file under the 600-line
//! conventions cap once α.1 added the edit-region pulse hook and the
//! persist-queue motion-timer arm.

use continuity_core::SelectionEdit;
use continuity_host::EditorOperation;

use crate::edit_trace;
use crate::editor_surface::selection_dispatch::SelectionDispatchEffects;
use crate::paint_trace::{is_trace_enabled, log_event, EventScope};
use crate::window_selection_adapter::NativeSelectionEditEffects;
use crate::Window;

impl Window {
    /// Dispatch a portable editor operation through the native surface path.
    /// Desktop-only operations never enter this enum.
    pub(crate) fn dispatch_editor_operation(
        &mut self,
        operation: EditorOperation,
    ) -> Result<(), continuity_command::Error> {
        match operation {
            EditorOperation::ApplySelectionEdit(edit) => self.dispatch_selection_edit(edit),
            EditorOperation::Undo => continuity_command::Context::undo(self),
            EditorOperation::Redo => continuity_command::Context::redo(self),
            EditorOperation::RedoAlternate => {
                continuity_command::Context::redo_alternate_branch(self)
            }
            _ => Err(continuity_command::Error::UnsupportedContext(
                "dispatch_editor_operation",
            )),
        }
    }

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
        let surface_effects = SelectionDispatchEffects::capture(&edit, pre.as_ref());
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
        let edit_pulse = surface_effects.apply_to(&mut self.surface.selection, self.buffer_id);
        self.apply_native_selection_edit_effects(NativeSelectionEditEffects {
            buffer_id: self.buffer_id,
            is_first_edit_since_paint,
            edit_pulse,
        });
        Ok(())
    }
}
