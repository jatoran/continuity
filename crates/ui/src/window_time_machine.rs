//! Time-machine slider state and named-snapshot label staging.
//!
//! The timeline overlay tracks its preview revision here. Snapshot labels
//! are mirrored into the editor thread so the next committed snapshot can
//! receive the staged label.
//!
//! Thread ownership: the UI thread of one window; [`Window`] is the only
//! mutator.

use continuity_buffer::Revision;

use crate::window::Window;

/// Per-window time-machine state.
#[derive(Debug, Clone, Default)]
pub struct TimeMachineState {
    /// The time-machine slider overlay is currently visible.
    pub timeline_visible: bool,
    /// Revision currently being previewed. `None` means the buffer head.
    pub timeline_preview_revision: Option<Revision>,
    /// Label staged for the next snapshot of the active buffer.
    pub pending_snapshot_label: Option<String>,
}

impl Window {
    /// Open the timeline overlay for the focused pane.
    pub(crate) fn open_buffer_timeline_impl(&mut self) -> Result<(), crate::Error> {
        self.view_options.time_machine.timeline_visible = true;
        self.view_options.time_machine.timeline_preview_revision = None;
        self.request_repaint();
        Ok(())
    }

    /// Stage `label` for the next snapshot; an empty label clears it.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_time_machine_state_is_quiescent() {
        let state = TimeMachineState::default();
        assert!(!state.timeline_visible);
        assert!(state.timeline_preview_revision.is_none());
        assert!(state.pending_snapshot_label.is_none());
    }

    #[test]
    fn pending_snapshot_label_round_trips() {
        let state = TimeMachineState {
            pending_snapshot_label: Some("pre-refactor".into()),
            ..Default::default()
        };
        assert_eq!(
            state.pending_snapshot_label.as_deref(),
            Some("pre-refactor")
        );
    }

    #[test]
    fn timeline_preview_revision_stages_a_value() {
        let state = TimeMachineState {
            timeline_preview_revision: Some(Revision(7)),
            ..Default::default()
        };
        assert_eq!(state.timeline_preview_revision, Some(Revision(7)));
    }
}
