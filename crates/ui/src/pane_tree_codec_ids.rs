//! ID reservation after pane-tree decode.
//!
//! Decoded pane/tab ids come from persisted state. The process-global fresh
//! counters must advance past them before runtime code creates new tabs or
//! panes, otherwise a new tab can overwrite a restored one with the same id.

use std::collections::HashMap;

use crate::pane_tree::{ClosedTab, Group, PaneId, Tab, TabId};

pub(crate) fn reserve_decoded_ids(
    groups: &HashMap<PaneId, Group>,
    tabs: &HashMap<TabId, Tab>,
    leaf_ids: &[PaneId],
    focused: PaneId,
    maximized: Option<PaneId>,
    recently_closed: &[ClosedTab],
) {
    let mut max_tab_id = tabs.keys().map(|id| id.0).max().unwrap_or(0);
    for group in groups.values() {
        max_tab_id = max_tab_id.max(group.active.0);
        for tab in group.tabs.iter().chain(group.mru.iter()) {
            max_tab_id = max_tab_id.max(tab.0);
        }
    }

    let mut max_pane_id = groups.keys().map(|id| id.0).max().unwrap_or(0);
    max_pane_id = max_pane_id.max(focused.0);
    if let Some(pane) = maximized {
        max_pane_id = max_pane_id.max(pane.0);
    }
    for pane in leaf_ids {
        max_pane_id = max_pane_id.max(pane.0);
    }
    for closed in recently_closed {
        if let Some(pane) = closed.origin_pane {
            max_pane_id = max_pane_id.max(pane.0);
        }
        if let Some(pane) = closed.parent_sibling_leaf {
            max_pane_id = max_pane_id.max(pane.0);
        }
    }

    TabId::reserve_at_least(max_tab_id.saturating_add(1));
    PaneId::reserve_at_least(max_pane_id.saturating_add(1));
}
