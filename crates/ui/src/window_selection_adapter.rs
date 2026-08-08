//! Desktop-only effects applied after a surface selection edit succeeds.

use continuity_buffer::BufferId;

use crate::editor_surface::selection_dispatch::EditPulseRequest;
use crate::Window;

/// Native adapter inputs produced by one successful surface edit dispatch.
pub(crate) struct NativeSelectionEditEffects {
    /// Buffer whose durable desktop representation changed.
    pub(crate) buffer_id: BufferId,
    /// Whether this is the first edit since the last paint.
    pub(crate) is_first_edit_since_paint: bool,
    /// Optional surface presentation request.
    pub(crate) edit_pulse: Option<EditPulseRequest>,
}

impl Window {
    /// Apply persistence, autosave, worker, and timer effects owned by the
    /// Windows desktop composition rather than by `EditorSurface`.
    pub(crate) fn apply_native_selection_edit_effects(
        &mut self,
        effects: NativeSelectionEditEffects,
    ) {
        self.schedule_vault_autosave(effects.buffer_id);
        self.cancel_active_display_prewarm();
        self.maybe_dispatch_projection_worker_early(
            effects.is_first_edit_since_paint,
            "selection_edit",
        );
        if let Some(request) = effects.edit_pulse {
            self.pulse_edit_region_after_dispatch(request.pre_caret_line, request.pre_line_count);
        }
        if self
            .persist_client
            .as_ref()
            .is_some_and(|client| client.unflushed_bytes() > 0)
        {
            self.start_motion_timer();
        }
    }
}
