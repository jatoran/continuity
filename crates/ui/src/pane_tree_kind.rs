//! Tab-kind discriminant + extension methods on [`Tab`] and
//! [`PaneTree`] — factored out of [`crate::pane_tree`] so that file
//! stays under the 600-line cap once [`TabKind::BufferHistory`] (and
//! the matching accessors) landed.
//!
//! No state of its own. Adding a new variant to [`TabKind`] is the
//! signal that the paint / input / persistence paths need a `match`
//! arm — keeping the variant list small is the point.

use continuity_buffer::BufferId;

use crate::pane_tree::{PaneTree, Tab};

/// Discriminant — what *kind* of tab this is.
///
/// The default is [`TabKind::Buffer`], i.e. a normal tab whose
/// `buffer_id` points at a real persisted buffer.
/// [`TabKind::BufferHistory`] is the non-buffer tab kind that renders
/// the swimlane visualization of every persisted buffer; its
/// `buffer_id` field carries [`BufferId::nil`] as a placeholder, and
/// the renderer projects per-tab state from
/// [`crate::buffer_history_tab::BufferHistoryTab`] keyed by `TabId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabKind {
    /// Standard tab: `buffer_id` references a buffer in editor state.
    #[default]
    Buffer,
    /// Buffer-history visualization tab: no underlying buffer, custom
    /// paint, custom input routing.
    BufferHistory,
}

impl Tab {
    /// Create a [`TabKind::BufferHistory`] tab.
    ///
    /// `buffer_id` is the **render-backing** synthetic empty buffer
    /// the regular paint pipeline reads behind the panel overlay
    /// (chrome / tab strip / status bar). Pass [`BufferId::nil`] only
    /// when the tab is a restore stub that has not yet had its
    /// synthetic buffer allocated — the paint path lazily adopts one
    /// on first frame and rewrites the field.
    ///
    /// The tab pins the default `"Buffer history"` label so the tab
    /// strip stays readable even when the synthetic buffer is empty.
    pub fn history(buffer_id: BufferId, created_at_ms: u64) -> Self {
        Self {
            id: crate::pane_tree::TabId::fresh(),
            kind: TabKind::BufferHistory,
            buffer_id,
            label_override: Some("Buffer history".to_string()),
            created_at_ms,
            file_associated: false,
            pinned: false,
        }
    }

    /// `Some(buffer_id)` when this tab maps to a real buffer, `None`
    /// for non-buffer kinds (e.g. [`TabKind::BufferHistory`]). The
    /// preferred accessor for any code path that reads
    /// `tab.buffer_id`.
    #[must_use]
    pub fn buffer_id_opt(&self) -> Option<BufferId> {
        match self.kind {
            TabKind::Buffer => Some(self.buffer_id),
            TabKind::BufferHistory => None,
        }
    }

    /// `true` when [`Self::kind`] is [`TabKind::Buffer`].
    #[must_use]
    pub fn is_buffer(&self) -> bool {
        matches!(self.kind, TabKind::Buffer)
    }

    /// `true` when [`Self::kind`] is [`TabKind::BufferHistory`].
    #[must_use]
    pub fn is_history(&self) -> bool {
        matches!(self.kind, TabKind::BufferHistory)
    }
}

impl PaneTree {
    /// Active buffer id wrapped in `Option`. `None` when the focused
    /// tab is a non-buffer kind. The preferred accessor for any path
    /// that should silently skip when no buffer is in focus (e.g. the
    /// decoration scheduler, file watchers).
    #[must_use]
    pub fn active_buffer_opt(&self) -> Option<BufferId> {
        let group = self.groups.get(&self.focused)?;
        self.tabs.get(&group.active).and_then(Tab::buffer_id_opt)
    }

    /// `Some(&Tab)` for the focused tab in the focused group. `None`
    /// only when invariants are violated (and the rest of the editor
    /// will already be broken).
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        let group = self.groups.get(&self.focused)?;
        self.tabs.get(&group.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::TabId;

    #[test]
    fn history_tab_carries_supplied_buffer_and_default_label() {
        let render_id = BufferId::new();
        let tab = Tab::history(render_id, 42);
        assert_eq!(tab.kind, TabKind::BufferHistory);
        assert_eq!(tab.buffer_id, render_id);
        assert!(tab.is_history());
        assert!(!tab.is_buffer());
        assert_eq!(tab.label_override.as_deref(), Some("Buffer history"));
        // `buffer_id_opt` is kind-gated, not nil-gated, so it returns
        // `None` even when the synthetic render buffer is non-nil —
        // callers that want to skip operations on the synthetic
        // buffer (file save, decoration, etc.) still get the right
        // answer.
        assert!(tab.buffer_id_opt().is_none());
    }

    #[test]
    fn buffer_tab_yields_some_id() {
        let id = BufferId::new();
        let tab = Tab::new(id, 0);
        assert_eq!(tab.kind, TabKind::Buffer);
        assert!(tab.is_buffer());
        assert!(!tab.is_history());
        assert_eq!(tab.buffer_id_opt(), Some(id));
    }

    #[test]
    fn pane_tree_active_buffer_opt_skips_history_tab() {
        let real = BufferId::new();
        let mut tree = PaneTree::singleton(real, 0);
        assert_eq!(tree.active_buffer_opt(), Some(real));
        let history = Tab::history(BufferId::new(), 7);
        let history_id = history.id;
        tree.tabs.insert(history_id, history);
        if let Some(group) = tree.groups.get_mut(&tree.focused) {
            group.push_tab(history_id, true);
        }
        assert!(tree.active_buffer_opt().is_none());
        let tab = tree.active_tab().expect("active tab present");
        assert_eq!(tab.id, history_id);
    }

    fn _unused_tab_id() -> TabId {
        TabId::fresh()
    }
}
