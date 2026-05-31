//! Repair pass for pane-tree structure and buffer tabs whose `BufferId`
//! is no longer live in the editor core.
//!
//! Thread ownership: callers run on a window's UI thread and mutate only
//! that window's `PaneTree`. Fresh replacement buffers are allocated by
//! sending `OpenBuffer` to the core thread, which remains the sole owner
//! of mutable buffer text.

use std::collections::HashSet;

use continuity_core::EditorHandle;

use crate::pane_tree::{Group, PaneId, PaneNode, PaneTree, Tab, TabId};

/// Repair pane/group/tab structural invariants that runtime code assumes.
///
/// This preserves the pane layout where possible. A leaf with no usable
/// tab receives a fresh empty buffer tab instead of becoming an invisible
/// or panic-prone pane.
pub(crate) fn repair_pane_tree_structure(tree: &mut PaneTree, editor: &EditorHandle) -> usize {
    let mut repaired = 0;
    let mut leaves = tree.root.leaf_ids();
    if leaves.is_empty() {
        let (pane, group, tab) = fresh_group(editor);
        tree.root = PaneNode::Leaf(pane);
        tree.groups.clear();
        tree.tabs.clear();
        tree.tabs.insert(tab.id, tab);
        tree.groups.insert(pane, group);
        tree.focused = pane;
        trace_structure_repair("empty_root", 1);
        return 1;
    }
    let mut seen_leaves = HashSet::new();
    leaves.retain(|pane| seen_leaves.insert(*pane));
    repaired += repair_leaf_groups(tree, editor, &leaves);
    repaired += remove_orphan_groups(tree, &leaves);
    repaired += repair_group_tabs(tree, editor, &leaves);
    if !tree.groups.contains_key(&tree.focused) {
        tree.focused = leaves[0];
        repaired += 1;
    }
    if tree
        .maximized
        .is_some_and(|pane| !tree.groups.contains_key(&pane))
    {
        tree.maximized = None;
        repaired += 1;
    }
    if repaired > 0 {
        trace_structure_repair("pane_tree", repaired);
    }
    repaired
}

fn repair_leaf_groups(tree: &mut PaneTree, editor: &EditorHandle, leaves: &[PaneId]) -> usize {
    let mut repaired = 0;
    for pane in leaves {
        if tree.groups.contains_key(pane) {
            continue;
        }
        let (_, mut group, mut tab) = fresh_group_for_pane(editor, *pane);
        while tree.tabs.contains_key(&tab.id) {
            tab = fresh_tab(editor);
            group.tabs = vec![tab.id];
            group.active = tab.id;
            group.mru = vec![tab.id];
        }
        tree.tabs.insert(tab.id, tab);
        tree.groups.insert(*pane, group);
        repaired += 1;
    }
    repaired
}

fn remove_orphan_groups(tree: &mut PaneTree, leaves: &[PaneId]) -> usize {
    let leaf_set: HashSet<PaneId> = leaves.iter().copied().collect();
    let orphan_groups: Vec<PaneId> = tree
        .groups
        .keys()
        .filter(|pane| !leaf_set.contains(pane))
        .copied()
        .collect();
    let mut repaired = 0;
    for pane in orphan_groups {
        if let Some(group) = tree.groups.remove(&pane) {
            for tab in group.tabs {
                tree.tabs.remove(&tab);
            }
            repaired += 1;
        }
    }
    repaired
}

fn repair_group_tabs(tree: &mut PaneTree, editor: &EditorHandle, leaves: &[PaneId]) -> usize {
    let mut repaired = 0;
    let mut assigned_tabs = HashSet::new();
    for pane in leaves {
        let valid_tabs: HashSet<TabId> = tree.tabs.keys().copied().collect();
        let Some(group) = tree.groups.get_mut(pane) else {
            continue;
        };
        let before_tabs = group.tabs.len();
        group
            .tabs
            .retain(|tab| valid_tabs.contains(tab) && assigned_tabs.insert(*tab));
        if group.tabs.len() != before_tabs {
            repaired += 1;
        }
        if group.tabs.is_empty() {
            let mut tab = fresh_tab(editor);
            while valid_tabs.contains(&tab.id) || assigned_tabs.contains(&tab.id) {
                tab = fresh_tab(editor);
            }
            let tab_id = tab.id;
            tree.tabs.insert(tab_id, tab);
            group.tabs.push(tab_id);
            assigned_tabs.insert(tab_id);
            repaired += 1;
        }
        let group_tabs = group.tabs.clone();
        let before_mru = group.mru.len();
        let mut seen_mru = HashSet::new();
        group
            .mru
            .retain(|tab| group_tabs.contains(tab) && seen_mru.insert(*tab));
        if group.mru.len() != before_mru {
            repaired += 1;
        }
        if !group.tabs.contains(&group.active) {
            group.active = group
                .mru
                .iter()
                .find(|tab| group.tabs.contains(tab))
                .copied()
                .unwrap_or(group.tabs[0]);
            repaired += 1;
        }
        if !group.mru.contains(&group.active) {
            group.mru.insert(0, group.active);
            repaired += 1;
        }
    }
    let orphan_tabs: Vec<TabId> = tree
        .tabs
        .keys()
        .filter(|tab| !assigned_tabs.contains(tab))
        .copied()
        .collect();
    for tab in orphan_tabs {
        tree.tabs.remove(&tab);
        repaired += 1;
    }
    repaired
}

fn fresh_group(editor: &EditorHandle) -> (PaneId, Group, Tab) {
    let tab = fresh_tab(editor);
    let tab_id = tab.id;
    let group = Group::singleton(tab_id);
    (group.id, group, tab)
}

fn fresh_group_for_pane(editor: &EditorHandle, pane: PaneId) -> (PaneId, Group, Tab) {
    let tab = fresh_tab(editor);
    let tab_id = tab.id;
    (
        pane,
        Group {
            id: pane,
            tabs: vec![tab_id],
            active: tab_id,
            mru: vec![tab_id],
        },
        tab,
    )
}

fn fresh_tab(editor: &EditorHandle) -> Tab {
    let buffer_id = editor.open_buffer("");
    Tab::new(buffer_id, 0)
}

/// Replace every buffer tab whose `BufferId` has no core snapshot with
/// a fresh empty buffer. Returns the number of repaired tabs.
pub(crate) fn repair_missing_buffer_tabs(tree: &mut PaneTree, editor: &EditorHandle) -> usize {
    let tabs: Vec<TabId> = tree
        .tabs
        .iter()
        .filter_map(|(id, tab)| {
            (tab.is_buffer() && editor.snapshot(tab.buffer_id).is_none()).then_some(*id)
        })
        .collect();
    let mut repaired = 0;
    for tab_id in tabs {
        if repair_buffer_tab(tree, editor, tab_id) {
            repaired += 1;
        }
    }
    repaired
}

/// Repair one tab when it is a buffer tab pointing at a missing core
/// snapshot. Returns `true` when the tab was rewritten.
pub(crate) fn repair_buffer_tab(tree: &mut PaneTree, editor: &EditorHandle, tab_id: TabId) -> bool {
    let Some(old_buffer) = tree.tabs.get(&tab_id).and_then(|tab| {
        (tab.is_buffer() && editor.snapshot(tab.buffer_id).is_none()).then_some(tab.buffer_id)
    }) else {
        return false;
    };
    let replacement = editor.open_buffer("");
    if let Some(tab) = tree.tabs.get_mut(&tab_id) {
        tab.buffer_id = replacement;
    }
    trace_repair(tab_id, old_buffer, replacement);
    true
}

fn trace_repair(
    tab_id: TabId,
    old_buffer: continuity_buffer::BufferId,
    replacement: continuity_buffer::BufferId,
) {
    if crate::paint_trace::is_trace_enabled() {
        crate::paint_trace::log_event(
            "pane_tree_buffer_repair",
            &format!(
                "tab={} old_buffer={} replacement_buffer={}",
                tab_id.0,
                old_buffer.as_uuid(),
                replacement.as_uuid()
            ),
        );
    }
}

fn trace_structure_repair(reason: &'static str, repaired: usize) {
    if crate::paint_trace::is_trace_enabled() {
        crate::paint_trace::log_event(
            "pane_tree_structure_repair",
            &format!("reason={reason} repairs={repaired}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use continuity_buffer::BufferId;
    use continuity_core::{EditorHandle, SystemClock};
    use continuity_persist::PersistHandle;

    use super::*;
    use crate::pane_shortcuts::{apply_layout, LayoutShortcut};
    use crate::pane_tree::PaneTree;

    fn editor() -> (tempfile::TempDir, PersistHandle, Arc<EditorHandle>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = PersistHandle::spawn(&dir.path().join("repair.db")).expect("persist");
        let editor = Arc::new(EditorHandle::spawn(persist.client(), Arc::new(SystemClock)));
        (dir, persist, editor)
    }

    #[test]
    fn repair_missing_buffer_tabs_replaces_unopened_buffers() {
        let (_dir, _persist, editor) = editor();
        let live = editor.open_buffer("live");
        let mut tree = PaneTree::singleton(live, 0);
        let missing = BufferId::new();
        let missing_tab = tree.open_tab_in_focused(missing, 1);

        assert!(editor.snapshot(missing).is_none());
        assert_eq!(repair_missing_buffer_tabs(&mut tree, &editor), 1);

        let repaired = tree.tabs[&missing_tab].buffer_id;
        assert_ne!(repaired, missing);
        assert!(editor.snapshot(repaired).is_some());
        assert_eq!(
            tree.tabs
                .values()
                .filter(|tab| tab.buffer_id == live)
                .count(),
            1
        );
    }

    #[test]
    fn repair_pane_tree_structure_replaces_missing_active_tab() {
        let (_dir, _persist, editor) = editor();
        let live = editor.open_buffer("live");
        let mut tree = PaneTree::singleton(live, 0);
        let active = tree.groups[&tree.focused].active;
        tree.tabs.remove(&active);

        assert!(tree.active_tab().is_none());
        assert!(repair_pane_tree_structure(&mut tree, &editor) > 0);

        let tab = tree.active_tab().expect("repair installs active tab");
        assert!(editor.snapshot(tab.buffer_id).is_some());
    }

    #[test]
    fn repair_pane_tree_structure_prunes_duplicate_tab_assignment() {
        let (_dir, _persist, editor) = editor();
        let live = editor.open_buffer("live");
        let mut tree = PaneTree::singleton(live, 0);
        let active = tree.groups[&tree.focused].active;
        let second = crate::pane_layout::split_focused(
            &mut tree,
            crate::pane_tree::SplitAxis::Horizontal,
            active,
            true,
        );

        assert_eq!(tree.groups[&second].active, active);
        assert!(repair_pane_tree_structure(&mut tree, &editor) > 0);

        for group in tree.groups.values() {
            let active_tab = tree.tabs.get(&group.active).expect("active tab exists");
            assert!(editor.snapshot(active_tab.buffer_id).is_some());
            assert!(group.tabs.contains(&group.active));
        }
    }

    #[test]
    fn repair_before_grid_layout_keeps_every_leaf_paintable() {
        let (_dir, _persist, editor) = editor();
        let live = editor.open_buffer("one");
        let mut tree = PaneTree::singleton(live, 0);
        for text in ["two", "three"] {
            let buffer_id = editor.open_buffer(text);
            tree.open_tab_in_focused(buffer_id, 0);
        }
        tree.open_tab_in_focused(BufferId::new(), 0);

        assert_eq!(repair_missing_buffer_tabs(&mut tree, &editor), 1);
        repair_pane_tree_structure(&mut tree, &editor);
        apply_layout(&mut tree, LayoutShortcut::Grid2x2);

        for group in tree.groups.values() {
            let active = tree.tabs[&group.active].buffer_id;
            assert!(
                editor.snapshot(active).is_some(),
                "active tab in every grid leaf must have a live snapshot"
            );
        }
    }
}
