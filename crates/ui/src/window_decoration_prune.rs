//! Block 2.1 — evict retained tree-sitter trees for off-screen buffers.
//!
//! A parsed `tree_sitter::Tree` costs ~35 MB for a 10k-line buffer (the
//! dominant per-buffer memory cost; see
//! `.docs/development/memory_optimization_plan.md`). The tree is retained
//! by the decoration worker pool's `BufferTreeCache` *only* to make the
//! next edit's reparse incremental — rendering reads the separately-cached
//! `Decorations`, never the tree. So we can drop the tree for any buffer
//! the user is not looking at and keep its decorations, leaving rendering
//! untouched; the buffer's next edit pays one full (non-incremental)
//! reparse on a worker thread — the same async path a cold buffer open
//! already takes, so there is no UI stall and no restyle flash on refocus.
//!
//! Thread ownership: the keep-set and the eviction decision are computed on
//! the UI thread (sole owner of the `PaneTree`). `DecoratePool::drop_buffer`
//! only enqueues into the worker-owned drop mailbox, so each worker remains
//! the sole writer of its `BufferTreeCache`.

use std::collections::HashSet;

use crate::Window;

/// Most-recently-used tabs in the focused pane (including the active tab)
/// whose trees we keep parsed even when not visible — an anti-thrash window
/// so rapid Ctrl+Tab cycling doesn't repeatedly drop and reparse.
const FOCUSED_PANE_MRU_KEEP: usize = 3;

impl Window {
    /// Drop the retained tree-sitter tree for every open buffer that is
    /// neither visible in a pane nor inside the focused pane's recent-MRU
    /// window. Decorations are left intact, so dropped buffers still render
    /// from cache; only their next edit pays a full reparse.
    ///
    /// Runs from the paint path, but only does work when the keep-set
    /// actually changes (pane focus / tab switch / close), so steady-state
    /// cost is one small set comparison per paint.
    pub(crate) fn prune_offscreen_decoration_trees(&self) {
        let Some(pool) = self.decorate_pool.as_ref() else {
            return;
        };

        // Keep-set: every visible buffer plus the focused pane's MRU window.
        let mut keep: HashSet<u128> = HashSet::new();
        for pane_id in self.tree.root.leaf_ids() {
            if let Some(group) = self.tree.groups.get(&pane_id) {
                if let Some(tab) = self.tree.tabs.get(&group.active) {
                    keep.insert(tab.buffer_id.as_uuid().as_u128());
                }
            }
        }
        if let Some(group) = self.tree.groups.get(&self.tree.focused) {
            for tab_id in group.mru.iter().take(FOCUSED_PANE_MRU_KEEP) {
                if let Some(tab) = self.tree.tabs.get(tab_id) {
                    keep.insert(tab.buffer_id.as_uuid().as_u128());
                }
            }
        }

        // Only act when the keep-set changed since the last prune.
        let mut keep_sorted: Vec<u128> = keep.iter().copied().collect();
        keep_sorted.sort_unstable();
        if *self.last_tree_prune_keep.borrow() == keep_sorted {
            return;
        }

        // Drop trees for every open buffer outside the keep-set. `drop_buffer`
        // is idempotent (a no-op for a buffer whose worker holds no tree), so
        // re-issuing across transitions is harmless.
        let mut handled: HashSet<u128> = HashSet::new();
        for tab in self.tree.tabs.values() {
            let id = tab.buffer_id.as_uuid().as_u128();
            if keep.contains(&id) || !handled.insert(id) {
                continue;
            }
            let _ = pool.drop_buffer(id);
        }

        *self.last_tree_prune_keep.borrow_mut() = keep_sorted;
    }
}
