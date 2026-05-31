//! Spec §6 layout-shortcut tree builders.
//!
//! Pure functions over [`crate::pane_tree::PaneTree`] — no rendering,
//! no Win32. Phase 13 keeps the shortcut set: single, two-cols, two-rows,
//! three-cols, four-cols, 2×2 grid, 2×4 grid.

use std::collections::HashSet;

use crate::pane_tree::{Group, PaneId, PaneNode, PaneTree, SplitAxis, TabId};

/// Spec §6 layout shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutShortcut {
    /// `Ctrl+Alt+1` — collapse to a single pane.
    Single,
    /// `Ctrl+Alt+2` — two columns.
    TwoCols,
    /// `Ctrl+Alt+Shift+2` — two rows.
    TwoRows,
    /// `Ctrl+Alt+3` — three columns.
    ThreeCols,
    /// `Ctrl+Alt+4` — four columns.
    FourCols,
    /// `Ctrl+Alt+5` — 2×2 grid.
    Grid2x2,
    /// `Ctrl+Alt+8` — 2×4 grid.
    Grid2x4,
}

impl LayoutShortcut {
    /// Number of leaf groups the layout produces.
    pub(crate) fn leaf_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::TwoCols | Self::TwoRows => 2,
            Self::ThreeCols => 3,
            Self::FourCols => 4,
            Self::Grid2x2 => 4,
            Self::Grid2x4 => 8,
        }
    }
}

/// Apply a layout shortcut to `tree`. Tabs from existing groups reflow
/// round-robin into the new groups; the focused group's active tab stays
/// in the *first* new group, which becomes the focused leaf.
pub fn apply_layout(tree: &mut PaneTree, shortcut: LayoutShortcut) {
    let tabs = collect_layout_tabs(tree);
    apply_layout_with_tabs(tree, shortcut, tabs);
}

/// Apply a layout shortcut using a caller-filtered tab order.
///
/// The first tab in `tabs` becomes the active tab in the first new group.
pub(crate) fn apply_layout_with_tabs(
    tree: &mut PaneTree,
    shortcut: LayoutShortcut,
    mut tabs: Vec<TabId>,
) {
    let target = shortcut.leaf_count();
    let mut seen = HashSet::new();
    tabs.retain(|tab| seen.insert(*tab) && tree.tabs.contains_key(tab));
    if tabs.is_empty() {
        return;
    }
    let first_tab = tabs[0];

    // Build `target` empty groups and distribute tabs round-robin.
    let mut new_groups: Vec<Group> = Vec::with_capacity(target);
    for _ in 0..target {
        new_groups.push(Group {
            id: PaneId::fresh(),
            tabs: Vec::new(),
            active: first_tab,
            mru: Vec::new(),
        });
    }

    for (i, tab) in tabs.iter().enumerate() {
        let bucket = i % target;
        let g = &mut new_groups[bucket];
        g.tabs.push(*tab);
        g.mru.push(*tab);
        if g.tabs.len() == 1 {
            g.active = *tab;
        }
    }

    new_groups.retain(|g| !g.tabs.is_empty());
    let focused_pane = new_groups[0].id;

    let assigned: HashSet<TabId> = new_groups
        .iter()
        .flat_map(|group| group.tabs.iter().copied())
        .collect();
    tree.tabs.retain(|tab_id, _| assigned.contains(tab_id));

    let leaves: Vec<PaneNode> = new_groups.iter().map(|g| PaneNode::Leaf(g.id)).collect();
    let root = match shortcut {
        LayoutShortcut::Single => leaves.into_iter().next().expect("at least one group"),
        LayoutShortcut::TwoCols | LayoutShortcut::ThreeCols | LayoutShortcut::FourCols => {
            even_split(SplitAxis::Horizontal, leaves)
        }
        LayoutShortcut::TwoRows => even_split(SplitAxis::Vertical, leaves),
        LayoutShortcut::Grid2x2 => grid(leaves, 2, 2),
        LayoutShortcut::Grid2x4 => grid(leaves, 2, 4),
    };

    tree.groups = new_groups.into_iter().map(|g| (g.id, g)).collect();
    tree.root = root;
    tree.focused = focused_pane;
    tree.maximized = None;
}

fn collect_layout_tabs(tree: &PaneTree) -> Vec<TabId> {
    let mut tabs: Vec<TabId> = Vec::new();
    let focused_active = tree.groups[&tree.focused].active;
    tabs.push(focused_active);
    for pid in tree.root.leaf_ids() {
        let g = &tree.groups[&pid];
        for &t in &g.tabs {
            if t != focused_active {
                tabs.push(t);
            }
        }
    }
    tabs
}

fn even_split(axis: SplitAxis, children: Vec<PaneNode>) -> PaneNode {
    if children.len() == 1 {
        return children.into_iter().next().expect("non-empty");
    }
    let n = children.len();
    let ratios = vec![1.0 / n as f32; n];
    PaneNode::Split {
        axis,
        ratios,
        children,
    }
}

fn grid(children: Vec<PaneNode>, rows: usize, cols: usize) -> PaneNode {
    if children.len() <= cols {
        return even_split(SplitAxis::Horizontal, children);
    }
    let mut row_nodes: Vec<PaneNode> = Vec::with_capacity(rows);
    let mut iter = children.into_iter().peekable();
    for _ in 0..rows {
        let mut row: Vec<PaneNode> = Vec::with_capacity(cols);
        for _ in 0..cols {
            if iter.peek().is_some() {
                row.push(iter.next().expect("peeked"));
            }
        }
        if !row.is_empty() {
            row_nodes.push(even_split(SplitAxis::Horizontal, row));
        }
    }
    even_split(SplitAxis::Vertical, row_nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::BufferId;

    fn make_tree(n: usize) -> PaneTree {
        let mut t = PaneTree::singleton(BufferId::new(), 0);
        for _ in 1..n {
            t.open_tab_in_focused(BufferId::new(), 0);
        }
        t
    }

    #[test]
    fn two_cols_layout_splits_evenly() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let leaves = t.root.leaf_ids();
        assert_eq!(leaves.len(), 2);
    }

    #[test]
    fn grid_2x2_round_robin_distributes_tabs() {
        let mut t = make_tree(8);
        apply_layout(&mut t, LayoutShortcut::Grid2x2);
        let leaves = t.root.leaf_ids();
        assert_eq!(leaves.len(), 4);
        for pid in &leaves {
            assert_eq!(t.groups[pid].tabs.len(), 2);
        }
    }

    #[test]
    fn grid_2x2_preserves_focused_tab_in_top_left() {
        // §E7: `pane.layout_grid_2x2` must keep the focused-pane active
        // tab in the top-left leaf (first new group after the layout
        // rebuild). The other tabs distribute round-robin into the
        // remaining leaves.
        let mut t = make_tree(4);
        let original_focused = t.groups[&t.focused].active;
        apply_layout(&mut t, LayoutShortcut::Grid2x2);
        // The first new leaf (top-left in Grid2x2) is `t.focused`.
        let top_left = t.focused;
        assert_eq!(t.groups[&top_left].tabs[0], original_focused);
        assert_eq!(t.groups[&top_left].active, original_focused);
    }

    #[test]
    fn fewer_tabs_than_layout_drops_empty_groups() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::FourCols);
        assert_eq!(t.root.leaf_ids().len(), 2);
    }

    #[test]
    fn leaf_count_matches_shortcut() {
        assert_eq!(LayoutShortcut::Single.leaf_count(), 1);
        assert_eq!(LayoutShortcut::TwoCols.leaf_count(), 2);
        assert_eq!(LayoutShortcut::TwoRows.leaf_count(), 2);
        assert_eq!(LayoutShortcut::ThreeCols.leaf_count(), 3);
        assert_eq!(LayoutShortcut::FourCols.leaf_count(), 4);
        assert_eq!(LayoutShortcut::Grid2x2.leaf_count(), 4);
        assert_eq!(LayoutShortcut::Grid2x4.leaf_count(), 8);
    }
}
