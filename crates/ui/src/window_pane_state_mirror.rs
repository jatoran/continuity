//! Focused-pane scalar save/load helpers.
//!
//! The focused pane is mirrored onto `Window` fields for the legacy
//! single-buffer paths. Right-edge chrome visibility is re-applied from
//! the focused buffer whenever the mirrored buffer changes, and the
//! paint-time geometry-anchor baselines are dropped so a baseline from
//! the previous pane's buffer is never compared against the new one.
//!
//! The mirror always describes exactly one tab, recorded in
//! `TabSessionState::adopted_tab`. Every swap of the mirror keeps that
//! record true: a saved pane state carries its own adopted tab, and a fresh
//! view bookmarks the outgoing tab before it replaces the mirror. A stale
//! record is what made a pane-focus switch bookmark the outgoing tab with
//! the incoming pane's scroll, so clicking back and forth between split
//! panes traded their scroll positions.
//!
//! Thread ownership: all state here is owned by the window UI thread.

use continuity_buffer::BufferId;

use crate::pane_layout::Rect;
use crate::pane_state::PerPaneState;
use crate::window::Window;

impl Window {
    /// Capture the current focused-pane scalar mirror for storage in
    /// `Window::panes`.
    pub(crate) fn capture_focused_pane_state(&self) -> PerPaneState {
        PerPaneState {
            buffer_id: self.buffer_id,
            view: self.surface.view.clone(),
            language: self.language,
            language_revision: self.language_revision,
            last_submitted_decoration_revision: self.last_submitted_decoration_revision,
            adopted_tab: self.tab_session.adopted_tab,
        }
    }

    /// Load a saved pane state into the focused scalar mirror.
    pub(crate) fn apply_pane_state(&mut self, state: PerPaneState) {
        self.buffer_id = state.buffer_id;
        self.surface.view = state.view;
        self.language = state.language;
        self.language_revision = state.language_revision;
        self.last_submitted_decoration_revision = state.last_submitted_decoration_revision;
        self.tab_session.adopted_tab = state.adopted_tab;
        self.surface.geometry_anchor.reset_baselines();
        self.apply_right_edge_chrome_for_current_view();
        self.clear_right_edge_layout_caches();
    }

    /// Initialize the focused scalar mirror for a pane that has no
    /// saved state yet. Right-edge chrome flags are loaded for `buffer_id`.
    ///
    /// Call after the focused group's active tab already names the tab this
    /// fresh view represents (a new tab, a reopened tab, a new split's tab,
    /// or a never-focused pane's active tab). The outgoing tab — the one the
    /// mirror still describes — is bookmarked first so switching back to it
    /// restores its scroll and selection.
    ///
    /// The fresh `ViewState` inherits the current global text scale (zoom)
    /// rather than resetting to 1.0, so opening a tab or split in an
    /// already-zoomed window keeps the zoom. Zoom is global, sourced from
    /// `[editor].text_scale`; a per-pane reset would visibly diverge.
    pub(crate) fn apply_new_pane_state(&mut self, buffer_id: BufferId) {
        self.save_tab_view_bookmark_for_adopted();
        self.tab_session.adopted_tab = self
            .tree
            .groups
            .get(&self.tree.focused)
            .map(|group| group.active);
        let scale = self.surface.view.font_size_scale;
        self.buffer_id = buffer_id;
        let mut view = continuity_layout::ViewState::new();
        view.font_size_scale = scale;
        self.surface.view = view;
        self.language = Self::default_language();
        self.language_revision = None;
        self.last_submitted_decoration_revision = None;
        self.surface.geometry_anchor.reset_baselines();
        self.apply_right_edge_chrome_for_current_view();
        self.clear_right_edge_layout_caches();
    }

    /// Take over the viewport geometry a pane was last painted at as a
    /// spectator, without moving its scroll. A spectator paints its saved
    /// scroll offset at its current body rect, so that rect — not the
    /// viewport the pane had when it was last focused — is the layout the
    /// user is looking at. Adopting it unanchored keeps a focus switch
    /// pixel-identical; anchoring against the stale focused-era viewport
    /// measured a wrap width the user never saw and shifted the view.
    pub(crate) fn adopt_spectator_viewport(&mut self, painted_body_rect: Rect) {
        self.surface.view.viewport_width_dip = painted_body_rect.w;
        self.surface.view.viewport_height_dip = painted_body_rect.h;
        self.sync_view_scroll_extent();
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
        let state = PerPaneState::new(BufferId::nil(), Language::Plain, None);
        assert_eq!(state.buffer_id, BufferId::nil());
        assert_eq!(state.language, Language::Plain);
        assert!(state.language_revision.is_none());
        assert!(state.adopted_tab.is_none());
    }
}
