//! Parent-split inspection helpers for the close/reopen flow.
//!
//! Lifted out of `pane_layout.rs` to keep that file under the
//! 600-line cap. Pure tree walks — no mutation, no rendering.

use crate::pane_tree::{PaneId, PaneNode, SplitAxis};

/// Parent-split summary captured at close time. Reopen reads `axis`
/// to know how the collapsed pane was originally split into the
/// surviving sibling tree, and `sibling_leaf` (the first leaf among
/// the collapsed pane's siblings) as the re-split anchor when the
/// origin pane is gone.
#[derive(Debug, Clone, Copy)]
pub struct ParentSplitInfo {
    /// Axis of the split that directly contained the closed pane.
    pub axis: SplitAxis,
    /// First leaf among the closed pane's siblings, in traversal
    /// order. `None` when all siblings are themselves splits.
    pub sibling_leaf: Option<PaneId>,
}

/// Walk the tree once to locate `pane`'s parent split and pick a
/// sibling-leaf anchor for reopen. Returns `None` when `pane` is the
/// root leaf (no parent split exists).
#[must_use]
pub fn find_parent_split_info(node: &PaneNode, pane: PaneId) -> Option<ParentSplitInfo> {
    match node {
        PaneNode::Leaf(_) => None,
        PaneNode::Split { axis, children, .. } => {
            let direct = children
                .iter()
                .position(|c| matches!(c, PaneNode::Leaf(id) if *id == pane));
            if let Some(idx) = direct {
                let sibling_leaf = children
                    .iter()
                    .enumerate()
                    .filter(|(other_idx, _)| *other_idx != idx)
                    .find_map(|(_, child)| first_leaf(child));
                return Some(ParentSplitInfo {
                    axis: *axis,
                    sibling_leaf,
                });
            }
            for c in children {
                if let Some(info) = find_parent_split_info(c, pane) {
                    return Some(info);
                }
            }
            None
        }
    }
}

fn first_leaf(node: &PaneNode) -> Option<PaneId> {
    match node {
        PaneNode::Leaf(id) => Some(*id),
        PaneNode::Split { children, .. } => children.iter().find_map(first_leaf),
    }
}

/// Stable token for the parent split axis used in trace lines.
#[must_use]
pub fn split_axis_token(axis: SplitAxis) -> &'static str {
    match axis {
        SplitAxis::Horizontal => "horizontal",
        SplitAxis::Vertical => "vertical",
    }
}
