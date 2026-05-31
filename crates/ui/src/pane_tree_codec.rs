//! JSON codec for [`PaneTree`].
//!
//! Phase 14 persists the per-window pane tree as a single JSON blob in the
//! `windows.pane_tree_json` column rather than a normalized
//! `panes`/`tabs`/`view_states` triple — the tree is always read or written
//! as a unit, and the recursive shape is awkward to query relationally.
//!
//! The wire format is private to this module; bumping it requires bumping
//! the persistence schema version.
//!
//! Single-writer rule: this module produces and consumes serialized strings
//! only; it never mutates a [`PaneTree`] outside the function that returns it.

use std::collections::{HashMap, HashSet};

use continuity_buffer::BufferId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pane_tree::{ClosedTab, Group, PaneId, PaneTree, SplitAxis, Tab, TabId};
use crate::pane_tree_kind::TabKind;

pub(super) type WireUuid = [u8; 16];

fn enc_uuid(u: Uuid) -> WireUuid {
    *u.as_bytes()
}

fn dec_uuid(b: WireUuid) -> Uuid {
    Uuid::from_bytes(b)
}

/// Errors produced while round-tripping a [`PaneTree`] through JSON.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// JSON-level decode error.
    #[error("pane_tree json: {0}")]
    Json(#[from] serde_json::Error),
    /// Structural validation failed (e.g., missing focused pane).
    #[error("pane_tree invalid: {0}")]
    Invalid(String),
}

/// Encode a [`PaneTree`] as JSON. Infallible (the in-memory structure is
/// always serializable).
#[must_use]
pub fn encode(tree: &PaneTree) -> String {
    let wire = WireTree::from_tree(tree);
    // Serialization of the wire shape only fails on writer errors, which
    // never happen with the `String` writer.
    serde_json::to_string(&wire).expect("invariant: WireTree always serializes")
}

/// Decode a [`PaneTree`] from a JSON blob produced by [`encode`].
///
/// # Errors
///
/// Returns [`CodecError::Json`] for malformed JSON or
/// [`CodecError::Invalid`] when the focused pane is missing from the
/// payload.
pub fn decode(json: &str) -> Result<PaneTree, CodecError> {
    let wire: WireTree = serde_json::from_str(json)?;
    wire.into_tree()
}

/// §H3 — encode a [`PaneTree`] together with the per-window
/// `folded_lines` set. Backwards compatible with [`encode`] / [`decode`]:
/// blobs written by either reader round-trip cleanly because
/// `folded_lines` carries `#[serde(default)]`.
#[must_use]
pub fn encode_with_folds(tree: &PaneTree, folded_lines: &[u32]) -> String {
    let wire = WireTree::from_tree_with_folds(tree, folded_lines);
    serde_json::to_string(&wire).expect("invariant: WireTree always serializes")
}

/// §H3 — decode both the [`PaneTree`] and the persisted `folded_lines`
/// from a JSON blob. Returns an empty `folded_lines` vector for older
/// blobs that predate the field.
///
/// # Errors
///
/// Returns [`CodecError::Json`] for malformed JSON or
/// [`CodecError::Invalid`] when the focused pane is missing.
pub fn decode_with_folds(json: &str) -> Result<(PaneTree, Vec<u32>), CodecError> {
    let wire: WireTree = serde_json::from_str(json)?;
    let folded_lines = wire.folded_lines.clone();
    let tree = wire.into_tree()?;
    Ok((tree, folded_lines))
}

/// F5 — encode the tree, fold state, AND per-buffer image expand
/// state into one JSON blob. Backwards compatible with
/// [`decode_with_folds`]: the new `image_expand_state` field is
/// stripped at decode time.
#[must_use]
pub fn encode_with_state(
    tree: &PaneTree,
    folded_lines: &[u32],
    image_expand_state: &std::collections::HashMap<(continuity_buffer::BufferId, usize), bool>,
) -> String {
    let wire = WireTree::from_tree_with_state(tree, folded_lines, image_expand_state);
    serde_json::to_string(&wire).expect("invariant: WireTree always serializes")
}

/// One persisted image-expand entry, as returned by
/// [`decode_with_state`]: `(buffer, source_byte, expanded)`.
pub type ImageExpandEntry = (continuity_buffer::BufferId, usize, bool);

/// F5 — decode the tree, fold state, AND per-buffer image expand
/// state. Older blobs without the new field decode with an empty
/// state vec.
///
/// # Errors
///
/// Returns [`CodecError::Json`] for malformed JSON or
/// [`CodecError::Invalid`] when the focused pane is missing.
pub fn decode_with_state(
    json: &str,
) -> Result<(PaneTree, Vec<u32>, Vec<ImageExpandEntry>), CodecError> {
    let wire: WireTree = serde_json::from_str(json)?;
    let folded_lines = wire.folded_lines.clone();
    let expand_state: Vec<ImageExpandEntry> = wire
        .image_expand_state
        .iter()
        .map(|e| {
            (
                continuity_buffer::BufferId::from_uuid(dec_uuid(e.buffer)),
                e.source_byte as usize,
                e.expanded,
            )
        })
        .collect();
    let tree = wire.into_tree()?;
    Ok((tree, folded_lines, expand_state))
}

/// Return every distinct buffer id referenced by encoded pane-tree tabs.
///
/// # Errors
///
/// Returns [`CodecError::Json`] when `json` is malformed.
pub fn buffer_ids_in_json(json: &str) -> Result<Vec<BufferId>, CodecError> {
    let wire: WireTree = serde_json::from_str(json)?;
    let mut out = Vec::new();
    for tab in wire.tabs {
        // Non-buffer tab kinds carry `BufferId::nil` as a placeholder;
        // skip them so the startup loader does not try to fetch a
        // buffer that never existed.
        if tab.kind != WireTabKind::Buffer {
            continue;
        }
        let id = BufferId::from_uuid(dec_uuid(tab.buffer));
        if id.is_nil() || out.contains(&id) {
            continue;
        }
        out.push(id);
    }
    Ok(out)
}

/// Return the active buffer id from encoded pane-tree JSON.
///
/// # Errors
///
/// Returns [`CodecError::Json`] when `json` is malformed or
/// [`CodecError::Invalid`] when the focused group / active tab reference
/// cannot be resolved.
pub fn active_buffer_id_in_json(json: &str) -> Result<BufferId, CodecError> {
    let wire: WireTree = serde_json::from_str(json)?;
    let focused = wire.focused;
    let group = wire
        .groups
        .iter()
        .find(|g| g.id == focused)
        .ok_or_else(|| CodecError::Invalid(format!("focused pane {focused} missing")))?;
    let active = group.active;
    let tab = wire
        .tabs
        .iter()
        .find(|t| t.id == active)
        .ok_or_else(|| CodecError::Invalid(format!("active tab {active} missing")))?;
    Ok(BufferId::from_uuid(dec_uuid(tab.buffer)))
}

#[derive(Serialize, Deserialize)]
struct WireTree {
    root: WireNode,
    groups: Vec<WireGroup>,
    tabs: Vec<WireTab>,
    focused: u64,
    #[serde(default)]
    recently_closed: Vec<WireClosed>,
    #[serde(default)]
    maximized: Option<u64>,
    /// §H3 — user-toggled source-line indices for indent folding.
    /// `u32::MAX` is the "fold all top-level" sentinel preserved verbatim.
    /// `#[serde(default)]` so older blobs round-trip without folds.
    #[serde(default)]
    folded_lines: Vec<u32>,
    /// F5 redesign — per-(buffer, URL) inline-image expand state. Each
    /// entry is one image whose user toggled it away from the default
    /// (collapsed). Entries with `expanded: false` are stripped at
    /// encode time so the wire shape stays small. Older blobs decode
    /// with an empty vector.
    #[serde(default)]
    image_expand_state: Vec<WireImageExpand>,
}

#[derive(Serialize)]
pub(super) struct WireImageExpand {
    pub(super) buffer: WireUuid,
    /// Source byte offset of the URL in the rope at save time.
    /// Renamed from the legacy `url` keying when expand state moved
    /// to per-occurrence (so two `![](images/x.png)` references in
    /// the same buffer no longer share a toggle). Legacy blobs that
    /// still carry the old `url` field are tolerated by the custom
    /// `Deserialize` impl in [`legacy`] — they decode with
    /// `source_byte = 0`.
    pub(super) source_byte: u64,
    pub(super) expanded: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WireNode {
    Leaf {
        pane: u64,
    },
    Split {
        axis: WireAxis,
        ratios: Vec<f32>,
        children: Vec<WireNode>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireAxis {
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize)]
struct WireGroup {
    id: u64,
    tabs: Vec<u64>,
    active: u64,
    mru: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
struct WireTab {
    id: u64,
    buffer: WireUuid,
    label_override: Option<String>,
    created_at_ms: u64,
    file_associated: bool,
    /// δ.1 — defaults to `false` for backward compat: blobs written
    /// before pinned-tab support deserialize cleanly.
    #[serde(default)]
    pinned: bool,
    /// Tab kind discriminant. Defaults to [`WireTabKind::Buffer`] for
    /// backward compat with blobs that predate non-buffer tab kinds.
    #[serde(default)]
    kind: WireTabKind,
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WireTabKind {
    #[default]
    Buffer,
    BufferHistory,
}

impl From<TabKind> for WireTabKind {
    fn from(k: TabKind) -> Self {
        match k {
            TabKind::Buffer => WireTabKind::Buffer,
            TabKind::BufferHistory => WireTabKind::BufferHistory,
        }
    }
}

impl From<WireTabKind> for TabKind {
    fn from(k: WireTabKind) -> Self {
        match k {
            WireTabKind::Buffer => TabKind::Buffer,
            WireTabKind::BufferHistory => TabKind::BufferHistory,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct WireClosed {
    buffer: WireUuid,
    label: String,
    closed_at_ms: u64,
    /// Pane id that hosted the tab when it was closed. `None` for
    /// records persisted before the field was added; reopen falls back
    /// to the focused pane in that case.
    #[serde(default)]
    origin_pane: Option<u64>,
    /// Axis of the parent split at close time. `"horizontal"` or
    /// `"vertical"`. Omitted in legacy records.
    #[serde(default)]
    parent_split_axis: Option<String>,
    /// First sibling leaf at close time. Used by reopen to re-split
    /// when `origin_pane` is collapsed.
    #[serde(default)]
    parent_sibling_leaf: Option<u64>,
}

impl WireTree {
    fn from_tree(tree: &PaneTree) -> Self {
        let root = WireNode::from_node(&tree.root);
        let groups = tree
            .groups
            .values()
            .map(|g| WireGroup {
                id: g.id.0,
                tabs: g.tabs.iter().map(|t| t.0).collect(),
                active: g.active.0,
                mru: g.mru.iter().map(|t| t.0).collect(),
            })
            .collect();
        let tabs = tree
            .tabs
            .values()
            .map(|t| WireTab {
                id: t.id.0,
                buffer: enc_uuid(t.buffer_id.as_uuid()),
                label_override: t.label_override.clone(),
                created_at_ms: t.created_at_ms,
                file_associated: t.file_associated,
                pinned: t.pinned,
                kind: t.kind.into(),
            })
            .collect();
        let recently_closed = tree
            .recently_closed
            .iter()
            .map(|c| WireClosed {
                buffer: enc_uuid(c.buffer_id.as_uuid()),
                label: c.label.clone(),
                closed_at_ms: c.closed_at_ms,
                origin_pane: c.origin_pane.map(|p| p.0),
                parent_split_axis: c.parent_split_axis.map(|a| match a {
                    SplitAxis::Horizontal => "horizontal".to_string(),
                    SplitAxis::Vertical => "vertical".to_string(),
                }),
                parent_sibling_leaf: c.parent_sibling_leaf.map(|p| p.0),
            })
            .collect();
        Self {
            root,
            groups,
            tabs,
            focused: tree.focused.0,
            recently_closed,
            maximized: tree.maximized.map(|p| p.0),
            folded_lines: Vec::new(),
            image_expand_state: Vec::new(),
        }
    }

    fn from_tree_with_folds(tree: &PaneTree, folded_lines: &[u32]) -> Self {
        let mut wire = Self::from_tree(tree);
        wire.folded_lines = folded_lines.to_vec();
        wire
    }

    /// F5 — encode the tree plus the per-buffer image expand state.
    fn from_tree_with_state(
        tree: &PaneTree,
        folded_lines: &[u32],
        image_expand_state: &std::collections::HashMap<(continuity_buffer::BufferId, usize), bool>,
    ) -> Self {
        let mut wire = Self::from_tree_with_folds(tree, folded_lines);
        wire.image_expand_state = image_expand_state
            .iter()
            .filter(|(_, expanded)| **expanded)
            .map(|((buf, source_byte), expanded)| WireImageExpand {
                buffer: enc_uuid(buf.as_uuid()),
                source_byte: *source_byte as u64,
                expanded: *expanded,
            })
            .collect();
        wire
    }

    fn into_tree(self) -> Result<PaneTree, CodecError> {
        let mut groups = HashMap::new();
        for g in self.groups {
            let id = PaneId(g.id);
            let mut seen_tabs = HashSet::new();
            let tabs = g
                .tabs
                .into_iter()
                .filter_map(|tab| {
                    let tab = TabId(tab);
                    seen_tabs.insert(tab).then_some(tab)
                })
                .collect();
            let mut seen_mru = HashSet::new();
            let mru = g
                .mru
                .into_iter()
                .filter_map(|tab| {
                    let tab = TabId(tab);
                    seen_mru.insert(tab).then_some(tab)
                })
                .collect();
            let previous = groups.insert(
                id,
                Group {
                    id,
                    tabs,
                    active: TabId(g.active),
                    mru,
                },
            );
            if previous.is_some() {
                return Err(CodecError::Invalid(format!(
                    "group {} appears more than once",
                    id.0
                )));
            }
        }
        let mut tabs = HashMap::new();
        for t in self.tabs {
            let id = TabId(t.id);
            let previous = tabs.insert(
                id,
                Tab {
                    id,
                    kind: t.kind.into(),
                    buffer_id: BufferId::from_uuid(dec_uuid(t.buffer)),
                    label_override: t.label_override,
                    created_at_ms: t.created_at_ms,
                    file_associated: t.file_associated,
                    pinned: t.pinned,
                },
            );
            if previous.is_some() {
                return Err(CodecError::Invalid(format!(
                    "tab {} appears more than once",
                    id.0
                )));
            }
        }
        let recently_closed: Vec<ClosedTab> = self
            .recently_closed
            .into_iter()
            .map(|c| ClosedTab {
                buffer_id: BufferId::from_uuid(dec_uuid(c.buffer)),
                label: c.label,
                closed_at_ms: c.closed_at_ms,
                origin_pane: c.origin_pane.map(PaneId),
                parent_split_axis: c.parent_split_axis.as_deref().and_then(|s| match s {
                    "horizontal" => Some(SplitAxis::Horizontal),
                    "vertical" => Some(SplitAxis::Vertical),
                    _ => None,
                }),
                parent_sibling_leaf: c.parent_sibling_leaf.map(PaneId),
            })
            .collect();
        let focused = PaneId(self.focused);
        if !groups.contains_key(&focused) {
            return Err(CodecError::Invalid(format!(
                "focused pane {} missing from groups",
                focused.0
            )));
        }
        let root = self.root.into_node();
        let leaf_ids = root.leaf_ids();
        if leaf_ids.is_empty() {
            return Err(CodecError::Invalid("pane tree has no leaves".to_string()));
        }
        let mut seen_leaves = HashSet::new();
        for leaf in &leaf_ids {
            if !seen_leaves.insert(*leaf) {
                return Err(CodecError::Invalid(format!(
                    "pane {} appears more than once in root",
                    leaf.0
                )));
            }
            if !groups.contains_key(leaf) {
                return Err(CodecError::Invalid(format!(
                    "root references pane {} missing from groups",
                    leaf.0
                )));
            }
        }
        for id in groups.keys() {
            if !seen_leaves.contains(id) {
                return Err(CodecError::Invalid(format!(
                    "group {} missing from root leaves",
                    id.0
                )));
            }
        }

        // Every group's tab lists must be self-contained. In particular,
        // `active` and MRU entries must be tabs in that group, not merely
        // tabs that exist elsewhere in the window. Partial state like this
        // can sit in `windows.pane_tree_json` after a crash mid-tab-close;
        // reject it here so restore falls back cleanly instead of carrying
        // a blank pane into runtime.
        let mut assigned_tabs = HashSet::new();
        for (id, g) in &groups {
            if g.tabs.is_empty() {
                return Err(CodecError::Invalid(format!("group {} has no tabs", id.0)));
            }
            if !tabs.contains_key(&g.active) {
                return Err(CodecError::Invalid(format!(
                    "group {} active tab {} missing from tabs",
                    id.0, g.active.0
                )));
            }
            if !g.tabs.contains(&g.active) {
                return Err(CodecError::Invalid(format!(
                    "group {} active tab {} missing from group tabs",
                    id.0, g.active.0
                )));
            }
            if let Some(missing) = g.tabs.iter().find(|t| !tabs.contains_key(t)) {
                return Err(CodecError::Invalid(format!(
                    "group {} references tab {} missing from tabs",
                    id.0, missing.0
                )));
            }
            for tab in &g.tabs {
                if !assigned_tabs.insert(*tab) {
                    return Err(CodecError::Invalid(format!(
                        "tab {} is assigned to more than one group",
                        tab.0
                    )));
                }
            }
            if let Some(missing) = g.mru.iter().find(|t| !g.tabs.contains(t)) {
                return Err(CodecError::Invalid(format!(
                    "group {} mru references tab {} outside group tabs",
                    id.0, missing.0
                )));
            }
        }
        if let Some(orphan) = tabs.keys().find(|tab| !assigned_tabs.contains(tab)) {
            return Err(CodecError::Invalid(format!(
                "tab {} missing from every group",
                orphan.0
            )));
        }
        let maximized = self.maximized.map(PaneId);
        if let Some(pane) = maximized {
            if !groups.contains_key(&pane) {
                return Err(CodecError::Invalid(format!(
                    "maximized pane {} missing from groups",
                    pane.0
                )));
            }
        }

        crate::pane_tree_codec_ids::reserve_decoded_ids(
            &groups,
            &tabs,
            &leaf_ids,
            focused,
            maximized,
            &recently_closed,
        );
        Ok(PaneTree {
            root,
            groups,
            tabs,
            focused,
            recently_closed,
            maximized,
        })
    }
}

pub mod legacy;

#[cfg(test)]
mod tests;
