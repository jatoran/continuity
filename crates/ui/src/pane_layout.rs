//! Geometric layout + spec §6 layout-shortcut tree builders.
//!
//! Pure functions over [`crate::pane_tree`]; no rendering.

use crate::pane_tree::{Group, PaneId, PaneNode, PaneTree, SplitAxis, TabId};

/// Axis-aligned rect in DIPs (top-left origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge in DIPs.
    pub x: f32,
    /// Top edge in DIPs.
    pub y: f32,
    /// Width in DIPs.
    pub w: f32,
    /// Height in DIPs.
    pub h: f32,
}

impl Rect {
    /// Convenience constructor.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge (`x + w`).
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge (`y + h`).
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// `true` iff `(px, py)` lies inside the rect (right/bottom exclusive).
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    /// Center point of the rect.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

/// Layout pixel constants used across the chrome + body layers.
pub mod metrics {
    /// Tab strip height in DIPs.
    pub(crate) const TAB_STRIP_HEIGHT_DIP: f32 = 28.0;
    /// Pane border thickness (drawn between siblings of a `Split`).
    pub const PANE_BORDER_DIP: f32 = 1.0;
    /// Minimum width or height for a leaf pane; layouts clamp below this.
    pub(crate) const MIN_LEAF_DIP: f32 = 80.0;
}

/// Compute per-leaf rects in document-traversal order.
///
/// `root_rect` is the area available to the tree (window client area minus
/// any window chrome). When `tree.maximized == Some(p)`, only `p` is laid
/// out and given the full root_rect.
pub fn compute_leaf_rects(tree: &PaneTree, root_rect: Rect) -> Vec<(PaneId, Rect)> {
    if let Some(maxed) = tree.maximized {
        if tree.groups.contains_key(&maxed) {
            return vec![(maxed, root_rect)];
        }
    }
    let mut out = Vec::new();
    walk(&tree.root, root_rect, &mut out);
    out
}

fn walk(node: &PaneNode, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        PaneNode::Leaf(id) => out.push((*id, rect)),
        PaneNode::Split {
            axis,
            ratios,
            children,
        } => {
            let total: f32 = ratios.iter().sum::<f32>().max(f32::EPSILON);
            match axis {
                SplitAxis::Horizontal => {
                    let mut x = rect.x;
                    let last = children.len().saturating_sub(1);
                    for (i, child) in children.iter().enumerate() {
                        let weight = ratios.get(i).copied().unwrap_or(0.0) / total;
                        let w = if i == last {
                            rect.right() - x
                        } else {
                            (rect.w * weight).max(metrics::MIN_LEAF_DIP)
                        };
                        let r = Rect::new(x, rect.y, w, rect.h);
                        walk(child, r, out);
                        x += w;
                    }
                }
                SplitAxis::Vertical => {
                    let mut y = rect.y;
                    let last = children.len().saturating_sub(1);
                    for (i, child) in children.iter().enumerate() {
                        let weight = ratios.get(i).copied().unwrap_or(0.0) / total;
                        let h = if i == last {
                            rect.bottom() - y
                        } else {
                            (rect.h * weight).max(metrics::MIN_LEAF_DIP)
                        };
                        let r = Rect::new(rect.x, y, rect.w, h);
                        walk(child, r, out);
                        y += h;
                    }
                }
            }
        }
    }
}

// `LayoutShortcut` and `apply_layout` live in `crate::pane_shortcuts`.

/// Direction for geometric pane focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    /// Move focus to the leaf immediately left of the current one.
    Left,
    /// Move focus to the leaf immediately right of the current one.
    Right,
    /// Move focus to the leaf immediately above the current one.
    Up,
    /// Move focus to the leaf immediately below the current one.
    Down,
}

/// Find the leaf adjacent to `from` in `direction`. Returns `None` if no
/// leaf lies in that direction. Geometric: nearest center-distance leaf
/// strictly past the from-pane's edge in that direction.
pub fn focus_geometric(
    tree: &PaneTree,
    root_rect: Rect,
    from: PaneId,
    direction: FocusDirection,
) -> Option<PaneId> {
    let rects = compute_leaf_rects(tree, root_rect);
    let from_rect = rects.iter().find(|(p, _)| *p == from)?.1;
    let (fx, fy) = from_rect.center();
    let mut best: Option<(PaneId, f32)> = None;
    for (pid, r) in rects.iter() {
        if *pid == from {
            continue;
        }
        let in_dir = match direction {
            FocusDirection::Left => r.right() <= from_rect.x + 1.0,
            FocusDirection::Right => r.x + 1.0 >= from_rect.right(),
            FocusDirection::Up => r.bottom() <= from_rect.y + 1.0,
            FocusDirection::Down => r.y + 1.0 >= from_rect.bottom(),
        };
        if !in_dir {
            continue;
        }
        let (cx, cy) = r.center();
        let dx = cx - fx;
        let dy = cy - fy;
        let dist = dx * dx + dy * dy;
        if best.is_none_or(|(_, d)| dist < d) {
            best = Some((*pid, dist));
        }
    }
    best.map(|(p, _)| p)
}

/// Resolve `(x, y)` to the `(PaneId, Rect)` containing it, if any.
pub(crate) fn pane_at_point(
    tree: &PaneTree,
    root_rect: Rect,
    x: f32,
    y: f32,
) -> Option<(PaneId, Rect)> {
    compute_leaf_rects(tree, root_rect)
        .into_iter()
        .find(|(_, r)| r.contains(x, y))
}

/// Split the focused leaf into two side-by-side leaves (axis = `Horizontal`)
/// or stacked (axis = `Vertical`). The new leaf becomes a sibling and gets
/// `new_buffer` as its only tab; focus moves to the new leaf.
pub fn split_focused(
    tree: &mut PaneTree,
    axis: SplitAxis,
    new_tab: TabId,
    new_pane_active_tab: bool,
) -> PaneId {
    // Build the new group + leaf.
    let mut new_group = Group::singleton_with_id(tree.fresh_unused_pane_id(), new_tab);
    let new_pane = new_group.id;
    if !new_pane_active_tab {
        // Caller wants tab present but not focused (rare; defensive).
        new_group.active = new_tab;
    }
    let leaf = PaneNode::Leaf(new_pane);
    tree.groups.insert(new_pane, new_group);

    let target_pane = tree.focused;
    splice_split(&mut tree.root, target_pane, axis, leaf);

    tree.focused = new_pane;
    tree.maximized = None;
    new_pane
}

fn splice_split(node: &mut PaneNode, target: PaneId, axis: SplitAxis, new_child: PaneNode) -> bool {
    match node {
        PaneNode::Leaf(id) if *id == target => {
            let old = std::mem::replace(node, PaneNode::Leaf(target));
            *node = PaneNode::Split {
                axis,
                ratios: vec![0.5, 0.5],
                children: vec![old, new_child],
            };
            true
        }
        PaneNode::Leaf(_) => false,
        PaneNode::Split { children, .. } => {
            for c in children.iter_mut() {
                if splice_split(c, target, axis, new_child.clone()) {
                    return true;
                }
            }
            false
        }
    }
}

/// Close a leaf pane. Its tabs are sent to the recently-closed list (with
/// labels resolved via the caller's `resolver`); the tree collapses so the
/// pane's parent split is replaced by the surviving sibling. Returns the
/// new focused pane, or `None` if the tree would become empty (the caller
/// must keep at least one pane alive).
pub fn close_pane<F: Fn(&crate::pane_tree::Tab) -> String>(
    tree: &mut PaneTree,
    pane: PaneId,
    now_ms: u64,
    label_resolver: &F,
) -> Option<PaneId> {
    if !tree.groups.contains_key(&pane) {
        return Some(tree.focused);
    }
    // Refuse to close the last leaf.
    let leaves = tree.root.leaf_ids();
    if leaves.len() <= 1 {
        return None;
    }

    // Capture parent-split info BEFORE the close mutates the tree so
    // reopen can restore the pane in its original tree position when
    // the collapse leaves the origin without an alive `PaneId`.
    let parent_info = find_parent_split_info(&tree.root, pane);

    // Move all the pane's tabs to the recently-closed list.
    let mut buffer_to_remember: Vec<crate::pane_tree::ClosedTab> = Vec::new();
    {
        let g = tree.groups.remove(&pane).expect("present");
        let cascade_tab_count = g.tabs.len();
        for tid in &g.tabs {
            if let Some(tab) = tree.tabs.remove(tid) {
                let label = label_resolver(&tab);
                if continuity_trace::is_enabled() {
                    continuity_trace::log_event(
                        "tab_close",
                        &format!(
                            "pane={} buffer={} tab={} label_len={} \
                             cascade_tab_count={} after=pane_collapse_cascade",
                            pane.0,
                            tab.buffer_id.as_uuid(),
                            tid.0,
                            label.chars().count(),
                            cascade_tab_count,
                        ),
                    );
                }
                buffer_to_remember.push(crate::pane_tree::ClosedTab {
                    buffer_id: tab.buffer_id,
                    label,
                    closed_at_ms: now_ms,
                    origin_pane: Some(pane),
                    parent_split_axis: parent_info.map(|p| p.axis),
                    parent_sibling_leaf: parent_info.and_then(|p| p.sibling_leaf),
                });
            }
        }
    }
    for ct in buffer_to_remember.into_iter().rev() {
        tree.recently_closed.insert(0, ct);
    }
    const RECENTLY_CLOSED_CAP: usize = 32;
    if tree.recently_closed.len() > RECENTLY_CLOSED_CAP {
        tree.recently_closed.truncate(RECENTLY_CLOSED_CAP);
    }

    if continuity_trace::is_enabled() {
        continuity_trace::log_event(
            "pane_close",
            &format!(
                "pane={} parent_axis={} parent_sibling_leaf={} \
                 recently_closed_len={}",
                pane.0,
                parent_info
                    .map(|p| split_axis_token(p.axis))
                    .unwrap_or("none"),
                parent_info
                    .and_then(|p| p.sibling_leaf)
                    .map(|s| s.0.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                tree.recently_closed.len(),
            ),
        );
    }

    remove_leaf(&mut tree.root, pane);

    // Pick a new focus: first remaining leaf in traversal order.
    let new_leaves = tree.root.leaf_ids();
    let next = new_leaves
        .first()
        .copied()
        .expect("non-empty after collapse");
    tree.focused = next;
    tree.maximized = None;
    Some(next)
}

pub mod parent_split;
use parent_split::{find_parent_split_info, split_axis_token};

/// Split the leaf `target_pane` into two children: the existing leaf
/// stays in place and the caller's `new_pane` becomes its sibling
/// along `axis`. The new leaf is inserted into the tree but the
/// caller is responsible for installing its `Group` into
/// `tree.groups`. Returns `true` when `target_pane` was found and
/// the split was applied.
pub fn splice_split_at_pane(
    tree: &mut PaneTree,
    target_pane: PaneId,
    axis: SplitAxis,
    new_pane: PaneId,
) -> bool {
    splice_split(&mut tree.root, target_pane, axis, PaneNode::Leaf(new_pane))
}

fn remove_leaf(node: &mut PaneNode, target: PaneId) -> bool {
    match node {
        PaneNode::Leaf(_) => false,
        PaneNode::Split {
            children, ratios, ..
        } => {
            // Direct child match: drop it.
            if let Some(idx) = children
                .iter()
                .position(|c| matches!(c, PaneNode::Leaf(id) if *id == target))
            {
                children.remove(idx);
                if idx < ratios.len() {
                    ratios.remove(idx);
                }
                // Collapse single-child split into the surviving child.
                if children.len() == 1 {
                    let surviving = children.remove(0);
                    *node = surviving;
                }
                return true;
            }
            // Otherwise recurse.
            let mut found = false;
            for c in children.iter_mut() {
                if remove_leaf(c, target) {
                    found = true;
                    break;
                }
            }
            if found {
                // After recursion, a child Split with one remaining grandchild
                // collapses up.
                for c in children.iter_mut() {
                    if let PaneNode::Split {
                        children: gc,
                        ratios: gr,
                        ..
                    } = c
                    {
                        if gc.len() == 1 && gr.len() == 1 {
                            let only = gc.remove(0);
                            *c = only;
                        }
                    }
                }
            }
            found
        }
    }
}

/// Resize the focused pane along `axis` by `delta` (DIPs) by adjusting the
/// nearest enclosing split's ratios. Positive `delta` grows the focused
/// pane at the expense of the next sibling; negative grows the previous
/// sibling.
pub fn resize_focused(tree: &mut PaneTree, axis: SplitAxis, delta_dip: f32, root_dim_dip: f32) {
    if root_dim_dip <= 0.0 || tree.root.leaf_ids().len() < 2 {
        return;
    }
    let target = tree.focused;
    let ratio_delta = delta_dip / root_dim_dip;
    nudge_ratio(&mut tree.root, target, axis, ratio_delta);
}

fn nudge_ratio(node: &mut PaneNode, target: PaneId, axis: SplitAxis, delta: f32) -> bool {
    match node {
        PaneNode::Leaf(id) => *id == target,
        PaneNode::Split {
            axis: a,
            ratios,
            children,
        } => {
            let local_axis = *a;
            let len = children.len();
            // Locate the child holding `target` first (immutable scan).
            let idx = children.iter().position(|c| node_contains(c, target));
            let i = match idx {
                Some(v) => v,
                None => return false,
            };
            if local_axis == axis && len >= 2 {
                let neighbor = if i + 1 < ratios.len() { i + 1 } else { i - 1 };
                let cap = 0.05;
                let new_self = (ratios[i] + delta).clamp(cap, 1.0 - cap);
                let actual_delta = new_self - ratios[i];
                ratios[i] = new_self;
                ratios[neighbor] = (ratios[neighbor] - actual_delta).max(cap);
                true
            } else {
                nudge_ratio(&mut children[i], target, axis, delta)
            }
        }
    }
}

fn node_contains(node: &PaneNode, target: PaneId) -> bool {
    match node {
        PaneNode::Leaf(id) => *id == target,
        PaneNode::Split { children, .. } => children.iter().any(|c| node_contains(c, target)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_shortcuts::{apply_layout, LayoutShortcut};
    use crate::pane_tree::PaneTree;
    use continuity_buffer::BufferId;

    fn make_tree(n: usize) -> PaneTree {
        let mut t = PaneTree::singleton(BufferId::new(), 0);
        for _ in 1..n {
            t.open_tab_in_focused(BufferId::new(), 0);
        }
        t
    }

    #[test]
    fn singleton_lays_out_full_rect() {
        let t = PaneTree::singleton(BufferId::new(), 0);
        let r = compute_leaf_rects(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, Rect::new(0.0, 0.0, 800.0, 600.0));
    }

    #[test]
    fn two_cols_layout_splits_evenly() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let r = compute_leaf_rects(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(r.len(), 2);
        assert!((r[0].1.w - 400.0).abs() < 0.01);
        assert!((r[1].1.w - 400.0).abs() < 0.01);
        assert_eq!(r[0].1.x, 0.0);
        assert_eq!(r[1].1.x, 400.0);
    }

    #[test]
    fn grid_2x2_round_robin_distributes_tabs() {
        let mut t = make_tree(8);
        apply_layout(&mut t, LayoutShortcut::Grid2x2);
        let leaves = t.root.leaf_ids();
        assert_eq!(leaves.len(), 4);
        // 8 tabs, 4 buckets → 2 tabs each.
        for pid in &leaves {
            assert_eq!(t.groups[pid].tabs.len(), 2);
        }
    }

    #[test]
    fn fewer_tabs_than_layout_drops_empty_groups() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::FourCols);
        // Only 2 tabs → only 2 leaves.
        assert_eq!(t.root.leaf_ids().len(), 2);
    }

    #[test]
    fn split_focused_inserts_sibling_and_focuses_new() {
        let mut t = PaneTree::singleton(BufferId::new(), 0);
        let original_pane = t.focused;
        let new_tab = t.open_tab_in_focused(BufferId::new(), 0);
        // Move the new tab into a split: first remove it from current group's
        // active position by simulating "split with new buffer".
        let pane = split_focused(&mut t, SplitAxis::Horizontal, new_tab, true);
        assert_ne!(pane, original_pane);
        assert_eq!(t.focused, pane);
        assert_eq!(t.root.leaf_ids().len(), 2);
    }

    #[test]
    fn close_pane_collapses_split_and_remembers() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let leaves = t.root.leaf_ids();
        assert_eq!(leaves.len(), 2);
        let to_close = leaves[1];
        let f = |t: &crate::pane_tree::Tab| format!("buf-{}", t.created_at_ms);
        let next = close_pane(&mut t, to_close, 1234, &f).expect("survives");
        assert_eq!(t.root.leaf_ids().len(), 1);
        assert_eq!(t.focused, next);
        assert_eq!(t.recently_closed.len(), 1);
    }

    #[test]
    fn close_last_pane_returns_none() {
        let mut t = PaneTree::singleton(BufferId::new(), 0);
        let only = t.focused;
        let f = |_: &crate::pane_tree::Tab| String::from("x");
        assert_eq!(close_pane(&mut t, only, 0, &f), None);
    }

    #[test]
    fn focus_geometric_finds_neighbor() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let root_rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let leaves = t.root.leaf_ids();
        let from = leaves[0];
        let right = focus_geometric(&t, root_rect, from, FocusDirection::Right);
        assert_eq!(right, Some(leaves[1]));
        let left = focus_geometric(&t, root_rect, leaves[1], FocusDirection::Left);
        assert_eq!(left, Some(leaves[0]));
        // Up/down on a horizontal layout finds nothing.
        assert_eq!(
            focus_geometric(&t, root_rect, from, FocusDirection::Up),
            None
        );
    }

    #[test]
    fn maximize_only_lays_out_focused() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let p = t.focused;
        t.maximized = Some(p);
        let r = compute_leaf_rects(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, p);
    }

    #[test]
    fn resize_grows_focused_at_expense_of_neighbor() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let leaves = t.root.leaf_ids();
        t.focus(leaves[0]);
        resize_focused(&mut t, SplitAxis::Horizontal, 80.0, 800.0);
        let rects = compute_leaf_rects(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        let w0 = rects[0].1.w;
        let w1 = rects[1].1.w;
        assert!(w0 > w1, "focused pane should grow: {w0} vs {w1}");
    }
}
