//! Per-pane runtime state owned by `ui::Window`.
//!
//! The focused pane's state is mirrored into `EditorSurface::view` plus the
//! `Window::buffer_id` / `Window::language*` fields. Non-focused panes' state
//! lives in [`Window::panes`] keyed by [`crate::pane_tree::PaneId`].
//!
//! Single-writer rule: only the owning UI thread writes the focused surface
//! view or any `PerPaneState` instance.

use continuity_buffer::BufferId;
use continuity_decorate::Language;
use continuity_layout::ViewState;

use crate::pane_tree::TabId;

/// Runtime state we keep for every leaf pane.
#[derive(Debug, Clone)]
pub struct PerPaneState {
    /// Active buffer for this pane (mirrors the focused tab's `buffer_id`).
    pub buffer_id: BufferId,
    /// Per-pane scroll / zoom / soft-wrap.
    pub view: ViewState,
    /// Cached language atom for the active buffer.
    pub language: Language,
    /// Revision the cached language was computed at.
    pub language_revision: Option<u64>,
    /// Revision last submitted to the decoration worker pool.
    pub last_submitted_decoration_revision: Option<u64>,
    /// Tab whose scroll / selection `view` describes. Restored into
    /// `TabSessionState::adopted_tab` when the pane regains focus, so a
    /// pane-focus switch is never mistaken for an in-pane tab switch (which
    /// would bookmark the wrong tab with the wrong pane's view). `None`
    /// when unknown; the next adopt then treats the pane's active tab as
    /// freshly shown.
    pub adopted_tab: Option<TabId>,
}

impl PerPaneState {
    /// Create a fresh per-pane state for `buffer_id` with a default view
    /// state, describing `adopted_tab`. The caller fills in viewport
    /// dimensions when the pane is laid out.
    pub fn new(buffer_id: BufferId, language: Language, adopted_tab: Option<TabId>) -> Self {
        Self {
            buffer_id,
            view: ViewState::new(),
            language,
            language_revision: None,
            last_submitted_decoration_revision: None,
            adopted_tab,
        }
    }
}
