//! Per-window persistence + multi-window helpers (Phase 14).
//!
//! `ui` doesn't depend on `persist`; the bridge is closure-based. The `app`
//! crate constructs a [`WindowPersistence`] whose `save` / `delete` closures
//! wrap the persist client, then attaches it to [`crate::WindowCommands`].
//!
//! Restoration likewise hands a [`RestoredState`] payload over —
//! deserialized in `app` — that the window applies during construction.
//!
//! Single-writer rule: every closure is called only from the window's UI
//! thread. The `app` registry reaches into a window's persistence only
//! through cross-thread channel sends to that thread.

use std::sync::Arc;

use continuity_buffer::WindowId;
use continuity_win::VirtualDesktopManager;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowPlacement, SetWindowPlacement, WINDOWPLACEMENT,
};

use crate::pane_tree::PaneTree;
use crate::pane_tree_codec;

/// Snapshot delivered to the persist sink at save time.
#[derive(Debug, Clone)]
pub struct WindowStateSnapshot {
    /// Serialized pane tree (see [`crate::pane_tree_codec`]).
    pub pane_tree_json: String,
    /// Virtual-desktop GUID, when known.
    pub virtual_desktop_guid: Option<[u8; 16]>,
    /// Best-effort monitor id (from `MonitorFromWindow`). `None` while the
    /// HWND is still being created.
    pub monitor_id: Option<i64>,
    /// Opaque `WINDOWPLACEMENT` blob produced by [`capture_placement`].
    pub placement_blob: Option<Vec<u8>>,
}

/// State delivered at restore time. All fields are best-effort: the window
/// silently falls back to defaults when any of them is absent or malformed.
#[derive(Debug, Clone)]
pub struct RestoredState {
    /// Encoded pane tree to decode and adopt.
    pub pane_tree_json: String,
    /// Last-seen virtual-desktop GUID. Replayed via
    /// `IVirtualDesktopManager::MoveWindowToDesktop` on creation.
    pub virtual_desktop_guid: Option<[u8; 16]>,
    /// Last-seen `WINDOWPLACEMENT` blob. Replayed via `SetWindowPlacement`.
    pub placement_blob: Option<Vec<u8>>,
}

/// Persistence callbacks attached to a window.
///
/// `save` is fired on graceful shutdown (`WM_DESTROY`) and on coarse-grained
/// state changes (pane manipulation, layout shortcut).
///
/// As of Phase 16.5, the row-tombstone-on-close decision lives in the
/// registry. A graceful `WM_DESTROY` produces `RegistryEvent::Closed`,
/// and the registry archives + tombstones the row for every such close,
/// including the last window. Only a crash (no `Closed` event) leaves the
/// row behind for the next launch to auto-restore.
#[derive(Clone)]
pub struct WindowPersistence {
    /// Stable window id (matches the persisted `windows.id`).
    pub window_id: WindowId,
    /// Initial state to replay during construction, if any.
    pub initial: Option<RestoredState>,
    /// Sink for state snapshots.
    pub save: Arc<dyn Fn(WindowStateSnapshot) + Send + Sync>,
}

impl std::fmt::Debug for WindowPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowPersistence")
            .field("window_id", &self.window_id)
            .field("initial.is_some", &self.initial.is_some())
            .finish()
    }
}

/// Build a [`PaneTree`] from a [`RestoredState`] payload, falling back to
/// the singleton tree when the JSON is malformed.
#[must_use]
pub fn restore_or_singleton(initial: Option<&RestoredState>, fallback_tree: PaneTree) -> PaneTree {
    let Some(state) = initial else {
        return fallback_tree;
    };
    match pane_tree_codec::decode(&state.pane_tree_json) {
        Ok(tree) => tree,
        Err(e) => {
            eprintln!(
                "continuity-ui: discarding restored pane tree (decode failed: {e}); falling back to singleton"
            );
            fallback_tree
        }
    }
}

/// §H3 — like [`restore_or_singleton`] but also returns the persisted
/// per-window `folded_lines` set. Returns `(tree, folded_lines)`. Old
/// blobs that predate the field decode with an empty `folded_lines`.
/// Out-of-range index validation (against the actual rope) is the
/// caller's responsibility — this function only decodes.
#[must_use]
pub fn restore_with_folds_or_singleton(
    initial: Option<&RestoredState>,
    fallback_tree: PaneTree,
) -> (PaneTree, Vec<u32>) {
    let Some(state) = initial else {
        return (fallback_tree, Vec::new());
    };
    match pane_tree_codec::decode_with_folds(&state.pane_tree_json) {
        Ok((tree, folds)) => (tree, folds),
        Err(e) => {
            eprintln!(
                "continuity-ui: discarding restored pane tree + folds (decode failed: {e}); falling back to singleton"
            );
            (fallback_tree, Vec::new())
        }
    }
}

/// F5 — like [`restore_with_folds_or_singleton`] but also returns
/// the persisted per-`(BufferId, source_byte)` image expand state.
/// Older blobs decode with an empty `Vec`. Decode failures fall back
/// to `(fallback_tree, [], [])`.
#[must_use]
pub fn restore_with_state_or_singleton(
    initial: Option<&RestoredState>,
    fallback_tree: PaneTree,
) -> (
    PaneTree,
    Vec<u32>,
    Vec<(continuity_buffer::BufferId, usize, bool)>,
) {
    let Some(state) = initial else {
        return (fallback_tree, Vec::new(), Vec::new());
    };
    match pane_tree_codec::decode_with_state(&state.pane_tree_json) {
        Ok((tree, folds, expand)) => (tree, folds, expand),
        Err(e) => {
            eprintln!(
                "continuity-ui: discarding restored pane tree + state (decode failed: {e}); falling back to singleton"
            );
            (fallback_tree, Vec::new(), Vec::new())
        }
    }
}

/// Capture the current window placement as an opaque byte blob suitable for
/// round-tripping through SQLite.
#[must_use]
pub(crate) fn capture_placement(hwnd: HWND) -> Option<Vec<u8>> {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetWindowPlacement(hwnd, &mut wp) }.is_ok();
    if !ok {
        return None;
    }
    let bytes: [u8; std::mem::size_of::<WINDOWPLACEMENT>()] = unsafe { std::mem::transmute(wp) };
    Some(bytes.to_vec())
}

/// Apply a placement blob produced by [`capture_placement`] to a window.
/// Returns `true` if the blob was the expected size and the call
/// succeeded; otherwise returns `false` (caller can fall back to default
/// CW_USEDEFAULT geometry).
pub(crate) fn apply_placement(hwnd: HWND, blob: &[u8]) -> bool {
    if blob.len() != std::mem::size_of::<WINDOWPLACEMENT>() {
        return false;
    }
    let mut bytes = [0u8; std::mem::size_of::<WINDOWPLACEMENT>()];
    bytes.copy_from_slice(blob);
    let wp: WINDOWPLACEMENT = unsafe { std::mem::transmute(bytes) };
    unsafe { SetWindowPlacement(hwnd, &wp) }.is_ok()
}

/// Try to record the current desktop GUID for `hwnd`. Returns `None` when
/// the manager hasn't been created yet or the call fails.
#[must_use]
pub(crate) fn current_desktop_guid(
    hwnd: HWND,
    manager: Option<&VirtualDesktopManager>,
) -> Option<[u8; 16]> {
    manager?.desktop_id_of_window(hwnd)
}

/// Move `hwnd` to the desktop named by `guid_bytes`, falling back silently
/// when the GUID is no longer present (per spec §6 "fall back to the active
/// desktop").
pub(crate) fn try_move_to_desktop(
    hwnd: HWND,
    manager: Option<&VirtualDesktopManager>,
    guid_bytes: [u8; 16],
) -> bool {
    let Some(m) = manager else {
        return false;
    };
    m.move_window_to_desktop(hwnd, guid_bytes)
        .unwrap_or_else(|e| {
            eprintln!("continuity-ui: MoveWindowToDesktop failed: {e}");
            false
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_buffer::BufferId;

    #[test]
    fn restore_falls_back_on_invalid_json() {
        let b = BufferId::new();
        let fallback = PaneTree::singleton(b, 0);
        let bad = RestoredState {
            pane_tree_json: "not json".into(),
            virtual_desktop_guid: None,
            placement_blob: None,
        };
        let restored = restore_or_singleton(Some(&bad), fallback);
        // We got the fallback singleton with the same active buffer.
        assert_eq!(restored.active_buffer(), b);
    }

    #[test]
    fn restore_uses_decoded_tree_when_valid() {
        let b = BufferId::new();
        let original = PaneTree::singleton(b, 0);
        let json = pane_tree_codec::encode(&original);
        let state = RestoredState {
            pane_tree_json: json,
            virtual_desktop_guid: None,
            placement_blob: None,
        };
        let other = BufferId::new();
        let fallback = PaneTree::singleton(other, 0);
        let restored = restore_or_singleton(Some(&state), fallback);
        assert_eq!(restored.active_buffer(), b);
    }
}
