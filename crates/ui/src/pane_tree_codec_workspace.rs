//! Backward-compatible folder-workspace fields in pane-tree JSON.

use std::collections::HashMap;

use continuity_buffer::BufferId;
use serde::{Deserialize, Serialize};

use crate::pane_tree::PaneTree;
use crate::pane_tree_codec::{dec_uuid, CodecError, ImageExpandEntry, WireTree};

/// Persisted folder-workspace chrome carried beside the pane tree.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceState {
    /// Opened folder or vault root.
    pub folder_root: Option<String>,
    /// Whether the left file tree was expanded.
    pub file_tree_visible: bool,
    /// User-resized file-tree width in DIPs.
    pub file_tree_width_dip: f32,
}

/// Encode pane state plus folder-workspace chrome.
#[must_use]
pub fn encode_with_workspace(
    tree: &PaneTree,
    folded_lines: &[u32],
    image_expand_state: &HashMap<(BufferId, usize), bool>,
    workspace: &WorkspaceState,
) -> String {
    let mut wire = WireTree::from_tree_with_state(tree, folded_lines, image_expand_state);
    wire.workspace = workspace.clone();
    serde_json::to_string(&wire).expect("invariant: WireTree always serializes")
}

/// Decode pane state plus backward-compatible folder-workspace chrome.
pub fn decode_with_workspace(
    json: &str,
) -> Result<(PaneTree, Vec<u32>, Vec<ImageExpandEntry>, WorkspaceState), CodecError> {
    let wire: WireTree = serde_json::from_str(json)?;
    let folded_lines = wire.folded_lines.clone();
    let expand_state = wire
        .image_expand_state
        .iter()
        .map(|entry| {
            (
                BufferId::from_uuid(dec_uuid(entry.buffer)),
                entry.source_byte as usize,
                entry.expanded,
            )
        })
        .collect();
    let workspace = wire.workspace.clone();
    let tree = wire.into_tree()?;
    Ok((tree, folded_lines, expand_state, workspace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_round_trips_and_old_json_defaults() {
        let tree = PaneTree::singleton(BufferId::new(), 0);
        let workspace = WorkspaceState {
            folder_root: Some(r"C:\notes".into()),
            file_tree_visible: true,
            file_tree_width_dip: 344.0,
        };
        let json = encode_with_workspace(&tree, &[], &HashMap::new(), &workspace);
        let (_, _, _, decoded) = decode_with_workspace(&json).expect("decode workspace");
        assert_eq!(decoded, workspace);

        let old_json = crate::pane_tree_codec::encode(&tree);
        let (_, _, _, decoded) = decode_with_workspace(&old_json).expect("decode old json");
        assert_eq!(decoded, WorkspaceState::default());
    }
}
