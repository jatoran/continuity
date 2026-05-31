//! Focused-pane scalar save/load helpers.
//!
//! The focused pane is mirrored onto `Window` fields for the legacy
//! single-buffer paths. Right-edge chrome visibility is re-applied from
//! the focused buffer whenever the mirrored buffer changes.
//!
//! Thread ownership: all state here is owned by the window UI thread.

use continuity_buffer::BufferId;

use crate::pane_state::PerPaneState;
use crate::window::Window;

impl Window {
    /// Capture the current focused-pane scalar mirror for storage in
    /// `Window::panes`.
    pub(crate) fn capture_focused_pane_state(&self) -> PerPaneState {
        PerPaneState {
            buffer_id: self.buffer_id,
            view: self.view.clone(),
            language: self.language,
            language_revision: self.language_revision,
            last_submitted_decoration_revision: self.last_submitted_decoration_revision,
        }
    }

    /// Load a saved pane state into the focused scalar mirror.
    pub(crate) fn apply_pane_state(&mut self, state: PerPaneState) {
        self.buffer_id = state.buffer_id;
        self.view = state.view;
        self.language = state.language;
        self.language_revision = state.language_revision;
        self.last_submitted_decoration_revision = state.last_submitted_decoration_revision;
        self.apply_right_edge_chrome_for_current_view();
        self.clear_right_edge_layout_caches();
    }

    /// Initialize the focused scalar mirror for a pane that has no
    /// saved state yet. Right-edge chrome flags are loaded for `buffer_id`.
    pub(crate) fn apply_new_pane_state(&mut self, buffer_id: BufferId) {
        self.buffer_id = buffer_id;
        self.view = continuity_layout::ViewState::new();
        self.language = Self::default_language();
        self.language_revision = None;
        self.last_submitted_decoration_revision = None;
        self.apply_right_edge_chrome_for_current_view();
        self.clear_right_edge_layout_caches();
    }

    /// Clear geometry caches that are tied to the current pane/body
    /// and buffer. Visibility flags remain unchanged.
    pub(crate) fn clear_right_edge_layout_caches(&mut self) {
        self.view_options.outline_layout = None;
        self.view_options.minimap_layout = None;
        self.view_options.search_minimap_layout = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::BufferId;
    use continuity_decorate::Language;

    #[test]
    fn new_per_pane_state_tracks_only_buffer_projection_state() {
        let state = PerPaneState::new(BufferId::nil(), Language::Plain);
        assert_eq!(state.buffer_id, BufferId::nil());
        assert_eq!(state.language, Language::Plain);
        assert!(state.language_revision.is_none());
    }
}
