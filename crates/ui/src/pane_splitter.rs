//! Splitter geometry + ratio mutation for Phase D3 mouse interaction.
//!
//! Walks the [`PaneTree`] to emit each boundary between siblings of a
//! `Split` node as a hit-testable [`Splitter`]. The renderer paints a
//! 1-DIP border between siblings (`metrics::PANE_BORDER_DIP`); the hit
//! zone expands to [`SPLITTER_HIT_HALF_DIP`] on either side so the user
//! can grab the line without sub-pixel precision.
//!
//! The split-resize math reuses `pane_layout::nudge_ratio` — passing any
//! leaf in the left/top branch as `target` resizes that exact split. A
//! separate [`equalize_split_for`] sets every child ratio in the matching
//! enclosing split to `1/n` for double-click "snap to even" behavior.

use crate::pane_layout::{metrics, Rect};
use crate::pane_tree::{PaneId, PaneNode, PaneTree, SplitAxis};

/// Half-width of the splitter hit zone, in DIPs. Total grabbable thickness
/// is `2 * SPLITTER_HIT_HALF_DIP` (centered on the painted border). 6 DIPs
/// each side ⇒ 12 DIP grab zone — comfortable target, matches what VS Code
/// uses for its splitters.
pub(crate) const SPLITTER_HIT_HALF_DIP: f32 = 6.0;

/// A single splitter boundary in a pane-tree split node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Splitter {
    /// Axis of the split that owns this boundary. `Horizontal` =
    /// side-by-side columns separated by a *vertical* line; `Vertical` =
    /// stacked rows separated by a *horizontal* line.
    pub axis: SplitAxis,
    /// Hit-testable rect for this boundary, expanded by
    /// [`SPLITTER_HIT_HALF_DIP`] on either side of the painted line.
    pub hit: Rect,
    /// First leaf id reachable by traversing the child branch immediately
    /// *left* (or *above*, for `Vertical`) of this splitter. Passing it
    /// to [`crate::pane_layout::resize_focused`] / `nudge_ratio` resizes
    /// the split this splitter owns.
    pub left_leaf: PaneId,
}

/// Walk the pane tree and emit every splitter (boundary between siblings
/// of a `Split` node). Maximize mode suppresses all splitters — a single
/// pane fills the root rect.
#[must_use]
pub(crate) fn splitters(tree: &PaneTree, root_rect: Rect) -> Vec<Splitter> {
    if tree.maximized.is_some() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk(&tree.root, root_rect, &mut out);
    out
}

fn walk(node: &PaneNode, rect: Rect, out: &mut Vec<Splitter>) {
    let PaneNode::Split {
        axis,
        ratios,
        children,
    } = node
    else {
        return;
    };
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
                let child_rect = Rect::new(x, rect.y, w, rect.h);
                walk(child, child_rect, out);
                // Emit a vertical splitter at the right edge for every
                // child except the last.
                if i < last {
                    let border_x = x + w;
                    let left = first_leaf(child);
                    out.push(Splitter {
                        axis: *axis,
                        hit: Rect::new(
                            border_x - SPLITTER_HIT_HALF_DIP,
                            rect.y,
                            SPLITTER_HIT_HALF_DIP * 2.0,
                            rect.h,
                        ),
                        left_leaf: left,
                    });
                }
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
                let child_rect = Rect::new(rect.x, y, rect.w, h);
                walk(child, child_rect, out);
                if i < last {
                    let border_y = y + h;
                    let top = first_leaf(child);
                    out.push(Splitter {
                        axis: *axis,
                        hit: Rect::new(
                            rect.x,
                            border_y - SPLITTER_HIT_HALF_DIP,
                            rect.w,
                            SPLITTER_HIT_HALF_DIP * 2.0,
                        ),
                        left_leaf: top,
                    });
                }
                y += h;
            }
        }
    }
}

fn first_leaf(node: &PaneNode) -> PaneId {
    match node {
        PaneNode::Leaf(id) => *id,
        PaneNode::Split { children, .. } => first_leaf(&children[0]),
    }
}

/// Set every child ratio at the nearest enclosing split of `axis`
/// containing `target` to `1/n` (equalize the row/column). No-op when
/// no matching enclosing split exists.
pub(crate) fn equalize_split_for(tree: &mut PaneTree, target: PaneId, axis: SplitAxis) -> bool {
    equalize_walk(&mut tree.root, target, axis)
}

fn equalize_walk(node: &mut PaneNode, target: PaneId, axis: SplitAxis) -> bool {
    // A bare Leaf cannot host an equalize: there's no enclosing split
    // along this branch. Only a Split with matching axis whose subtree
    // contains `target` returns true.
    let PaneNode::Split {
        axis: a,
        ratios,
        children,
    } = node
    else {
        return false;
    };
    let local_axis = *a;
    let Some(i) = children.iter().position(|c| node_contains(c, target)) else {
        return false;
    };
    if local_axis == axis {
        let n = ratios.len().max(1) as f32;
        let even = 1.0 / n;
        for r in ratios.iter_mut() {
            *r = even;
        }
        return true;
    }
    equalize_walk(&mut children[i], target, axis)
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
    use crate::pane_layout::compute_leaf_rects;
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
    fn singleton_emits_no_splitters() {
        let t = PaneTree::singleton(BufferId::new(), 0);
        let s = splitters(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert!(s.is_empty());
    }

    #[test]
    fn two_cols_emits_one_vertical_splitter() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let s = splitters(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].axis, SplitAxis::Horizontal);
        // Splitter hit-zone straddles x=400.
        assert!(s[0].hit.x < 400.0 && s[0].hit.right() > 400.0);
        assert!(s[0].hit.w <= SPLITTER_HIT_HALF_DIP * 2.0 + 0.01);
    }

    #[test]
    fn two_rows_emits_one_horizontal_splitter() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoRows);
        let s = splitters(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].axis, SplitAxis::Vertical);
        assert!(s[0].hit.y < 300.0 && s[0].hit.bottom() > 300.0);
    }

    #[test]
    fn grid_2x2_emits_three_splitters() {
        let mut t = make_tree(4);
        apply_layout(&mut t, LayoutShortcut::Grid2x2);
        let s = splitters(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        // 2 col splitters (one per row of cells) + 1 row splitter at root.
        // Grid is built as Vertical(Horizontal, Horizontal) → 1 vertical
        // splitter at root y=300, 2 horizontal splitters at row-x=400.
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn maximized_suppresses_splitters() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        t.maximized = Some(t.focused);
        let s = splitters(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert!(s.is_empty());
    }

    #[test]
    fn left_leaf_is_left_branch_leaf() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let leaves = t.root.leaf_ids();
        let s = splitters(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(s[0].left_leaf, leaves[0]);
    }

    #[test]
    fn equalize_resets_ratios_to_even() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        // Skew the split first via nudge_ratio.
        let target = t.focused;
        crate::pane_layout::resize_focused(&mut t, SplitAxis::Horizontal, 100.0, 800.0);
        // Now equalize via any leaf in left branch.
        let leaves = t.root.leaf_ids();
        let left = leaves[0];
        assert!(equalize_split_for(&mut t, left, SplitAxis::Horizontal));
        // Both columns equal.
        let rects = compute_leaf_rects(&t, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert!((rects[0].1.w - rects[1].1.w).abs() < 0.01);
        let _ = target;
    }

    #[test]
    fn equalize_unknown_pane_returns_false() {
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let phantom = PaneId::fresh();
        assert!(!equalize_split_for(&mut t, phantom, SplitAxis::Horizontal));
    }

    #[test]
    fn equalize_wrong_axis_walks_into_child() {
        // Two cols → no enclosing Vertical split → false.
        let mut t = make_tree(2);
        apply_layout(&mut t, LayoutShortcut::TwoCols);
        let leaves = t.root.leaf_ids();
        assert!(!equalize_split_for(&mut t, leaves[0], SplitAxis::Vertical));
    }
}
