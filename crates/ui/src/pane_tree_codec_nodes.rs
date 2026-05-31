//! Recursive pane-tree node conversion for the JSON codec.

use crate::pane_tree::{PaneId, PaneNode, SplitAxis};
use crate::pane_tree_codec::{WireAxis, WireNode};

impl WireNode {
    pub(crate) fn from_node(n: &PaneNode) -> Self {
        match n {
            PaneNode::Leaf(id) => WireNode::Leaf { pane: id.0 },
            PaneNode::Split {
                axis,
                ratios,
                children,
            } => WireNode::Split {
                axis: WireAxis::from(*axis),
                ratios: ratios.clone(),
                children: children.iter().map(WireNode::from_node).collect(),
            },
        }
    }

    pub(crate) fn into_node(self) -> PaneNode {
        match self {
            WireNode::Leaf { pane } => PaneNode::Leaf(PaneId(pane)),
            WireNode::Split {
                axis,
                ratios,
                children,
            } => PaneNode::Split {
                axis: axis.into(),
                ratios,
                children: children.into_iter().map(WireNode::into_node).collect(),
            },
        }
    }
}

impl From<SplitAxis> for WireAxis {
    fn from(a: SplitAxis) -> Self {
        match a {
            SplitAxis::Horizontal => WireAxis::Horizontal,
            SplitAxis::Vertical => WireAxis::Vertical,
        }
    }
}

impl From<WireAxis> for SplitAxis {
    fn from(a: WireAxis) -> Self {
        match a {
            WireAxis::Horizontal => SplitAxis::Horizontal,
            WireAxis::Vertical => SplitAxis::Vertical,
        }
    }
}
