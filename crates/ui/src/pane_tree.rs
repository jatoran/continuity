//! Pane tree + tab metadata for a single window.
//!
//! Per spec §6: a `PaneNode` is a recursive tree of `Split { axis, ratio,
//! children }` and `Group { tabs, active, mru }`. Each `Group` is a leaf —
//! it shows a tab strip and a single editor body for the active tab.
//!
//! The model is purely in-memory; persistence (per Phase 14) reads/writes
//! into `windows`/`panes`/`tabs` tables and round-trips this structure.
//!
//! Single-writer rule: the owning `ui::Window`'s UI thread is the only
//! writer of any `PaneTree` instance.

use continuity_buffer::BufferId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::pane_tree_kind::TabKind;

/// Identifier for a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub u64);

/// Identifier for a leaf `Group` in the pane tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PANE_ID: AtomicU64 = AtomicU64::new(1);

fn reserve_counter_at_least(counter: &AtomicU64, next: u64) {
    let mut current = counter.load(Ordering::Relaxed);
    while current < next {
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

impl TabId {
    /// Allocate a fresh tab id from the process-global counter.
    pub fn fresh() -> Self {
        Self(NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Ensure future fresh tab ids are greater than or equal to `next`.
    pub(crate) fn reserve_at_least(next: u64) {
        reserve_counter_at_least(&NEXT_TAB_ID, next);
    }
}

impl PaneId {
    /// Allocate a fresh pane id from the process-global counter.
    pub fn fresh() -> Self {
        Self(NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Ensure future fresh pane ids are greater than or equal to `next`.
    pub(crate) fn reserve_at_least(next: u64) {
        reserve_counter_at_least(&NEXT_PANE_ID, next);
    }
}

/// A single tab (one buffer + label state, or a non-buffer surface
/// keyed by `kind`).
#[derive(Debug, Clone)]
pub struct Tab {
    /// Stable tab id.
    pub id: TabId,
    /// What kind of tab this is. See [`TabKind`].
    pub kind: TabKind,
    /// For [`TabKind::Buffer`], the buffer shown when this tab is
    /// active. For [`TabKind::BufferHistory`], the placeholder
    /// [`BufferId::nil`] (the tab has no underlying buffer).
    ///
    /// Most call sites should prefer [`Self::buffer_id_opt`] over
    /// reading this field directly, so that a future tab kind without
    /// a buffer slot can be added without revisiting every read.
    pub buffer_id: BufferId,
    /// User-set label override. `None` falls through to first-non-empty-line
    /// or "Untitled" — see [`resolve_label`].
    pub label_override: Option<String>,
    /// Wall-clock millis at creation.
    pub created_at_ms: u64,
    /// `true` once the tab is associated with a file path on disk (Phase 15).
    /// Today this is always `false` and the close path always goes through
    /// trash.
    pub file_associated: bool,
    /// δ.1 — `true` when this tab is pinned. Pinned tabs are
    /// rendered leftmost, prefixed with a pin glyph, and are exempt
    /// from any "close others" / mass-close pass. They are still
    /// individually closable by the user.
    pub pinned: bool,
}

impl Tab {
    /// Create a `Buffer`-kind tab for `buffer_id` with no label override.
    pub fn new(buffer_id: BufferId, created_at_ms: u64) -> Self {
        Self::with_id(TabId::fresh(), buffer_id, created_at_ms)
    }

    /// Create a `Buffer`-kind tab with a caller-selected id.
    pub(crate) fn with_id(id: TabId, buffer_id: BufferId, created_at_ms: u64) -> Self {
        Self {
            id,
            kind: TabKind::Buffer,
            buffer_id,
            label_override: None,
            created_at_ms,
            file_associated: false,
            pinned: false,
        }
    }
}

// Tab-label resolution moved to `pane_tree_label.rs` (Phase H6 split,
// 2026-05-13) to keep this file under the 600-line cap. Re-exported
// for the existing pane-tree callers.
pub(crate) use crate::pane_tree_label::resolve_label;

// TabKind enum + per-tab predicate methods + the `_opt` accessors on
// PaneTree live in `pane_tree_kind.rs`; callers import the type from
// there directly (`crate::pane_tree_kind::TabKind`).

/// Split orientation: `Horizontal` arranges children left→right (vertical
/// borders between them); `Vertical` arranges children top→bottom.
///
/// (The spec's "split horizontal" command in §6 means "split the pane into
/// two columns" — i.e. side-by-side, axis = `Horizontal`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    /// Children arranged left-to-right (vertical separators).
    Horizontal,
    /// Children arranged top-to-bottom (horizontal separators).
    Vertical,
}

/// Recursive pane node. Either a leaf `Group` or a `Split` with `>=2` children.
#[derive(Debug, Clone)]
pub enum PaneNode {
    /// Internal split — `ratios.len() == children.len()` and `ratios.iter().sum() ~= 1.0`.
    Split {
        /// Split orientation.
        axis: SplitAxis,
        /// Per-child fractional weight of the split rect.
        ratios: Vec<f32>,
        /// Children in the same order as `ratios`.
        children: Vec<PaneNode>,
    },
    /// Leaf — references a `Group` by id; group payload lives in
    /// [`PaneTree::groups`].
    Leaf(PaneId),
}

impl PaneNode {
    /// Walk every leaf in left-to-right / top-to-bottom traversal order.
    pub(crate) fn for_each_leaf<F: FnMut(PaneId)>(&self, f: &mut F) {
        match self {
            PaneNode::Leaf(id) => f(*id),
            PaneNode::Split { children, .. } => {
                for c in children {
                    c.for_each_leaf(f);
                }
            }
        }
    }

    /// Collect leaf ids in traversal order.
    pub fn leaf_ids(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.for_each_leaf(&mut |id| out.push(id));
        out
    }
}

/// Group payload (a leaf in the pane tree).
#[derive(Debug, Clone)]
pub struct Group {
    /// Stable id matching a `PaneNode::Leaf`.
    pub id: PaneId,
    /// Tab order (positional, left-to-right).
    pub tabs: Vec<TabId>,
    /// Currently visible tab.
    pub active: TabId,
    /// Most-recently-used stack (front = most recent). Includes `active`
    /// as the first element after every focus change.
    pub mru: Vec<TabId>,
}

impl Group {
    /// New group with a single tab.
    pub fn singleton(tab: TabId) -> Self {
        Self::singleton_with_id(PaneId::fresh(), tab)
    }

    /// New group with a single tab and a caller-selected pane id.
    pub(crate) fn singleton_with_id(id: PaneId, tab: TabId) -> Self {
        Self {
            id,
            tabs: vec![tab],
            active: tab,
            mru: vec![tab],
        }
    }

    /// Insert `tab` at `index` (clamped); if `make_active` is true, focus it.
    pub(crate) fn insert_tab(&mut self, tab: TabId, index: usize, make_active: bool) {
        let i = index.min(self.tabs.len());
        self.tabs.insert(i, tab);
        if make_active {
            self.activate(tab);
        } else {
            // New tabs land at the back of MRU until activated.
            self.mru.push(tab);
        }
    }

    /// Push a tab to the back of the positional order.
    pub fn push_tab(&mut self, tab: TabId, make_active: bool) {
        self.insert_tab(tab, self.tabs.len(), make_active);
    }

    /// Mark `tab` as the active tab and bump it to the front of MRU.
    pub fn activate(&mut self, tab: TabId) {
        if !self.tabs.contains(&tab) {
            return;
        }
        self.active = tab;
        self.mru.retain(|&t| t != tab);
        self.mru.insert(0, tab);
    }

    /// Remove `tab` from this group. Returns the new active tab id, or
    /// `None` if the group is now empty.
    pub(crate) fn remove_tab(&mut self, tab: TabId) -> Option<TabId> {
        let pos = self.tabs.iter().position(|&t| t == tab)?;
        self.tabs.remove(pos);
        self.mru.retain(|&t| t != tab);
        let current_tabs = self.tabs.clone();
        self.mru.retain(|t| current_tabs.contains(t));
        if self.tabs.is_empty() {
            return None;
        }
        if self.active == tab {
            // Activate the next MRU; fall back to the position-neighbor.
            let next = self
                .mru
                .iter()
                .find(|t| self.tabs.contains(t))
                .copied()
                .unwrap_or_else(|| self.tabs[pos.min(self.tabs.len() - 1)]);
            self.active = next;
            // Make sure MRU front == active.
            self.mru.retain(|&t| t != next);
            self.mru.insert(0, next);
        } else if !self.mru.contains(&self.active) {
            self.mru.insert(0, self.active);
        }
        Some(self.active)
    }

    /// Reorder `tab` to `new_index` (clamped). Other tabs slide to fill
    /// the gap. Active id + MRU stack are preserved. Returns `true` when
    /// the position actually changed.
    pub(crate) fn reorder_tab(&mut self, tab: TabId, new_index: usize) -> bool {
        let Some(old_pos) = self.tabs.iter().position(|&t| t == tab) else {
            return false;
        };
        let target = new_index.min(self.tabs.len().saturating_sub(1));
        if old_pos == target {
            return false;
        }
        self.tabs.remove(old_pos);
        self.tabs.insert(target, tab);
        true
    }

    /// Step to the next positional tab, wrapping.
    pub fn step_positional(&mut self, delta: i32) {
        if self.tabs.len() < 2 {
            return;
        }
        let cur = match self.tabs.iter().position(|&t| t == self.active) {
            Some(p) => p,
            None => return,
        };
        let len = self.tabs.len() as i32;
        let next = (((cur as i32) + delta).rem_euclid(len)) as usize;
        self.activate(self.tabs[next]);
    }

    /// Step through the MRU stack (Ctrl+Tab semantics: pick the second-most-
    /// recent).
    pub fn step_mru(&mut self, delta: i32) {
        if self.mru.len() < 2 {
            return;
        }
        let len = self.mru.len() as i32;
        // delta=+1 → second entry; delta=-1 → last entry.
        let target = (delta).rem_euclid(len) as usize;
        let tab = self.mru[target];
        self.activate(tab);
    }

    /// Activate the 1-indexed positional tab. Returns `true` when applicable.
    pub(crate) fn activate_positional(&mut self, one_indexed: usize) -> bool {
        if one_indexed == 0 || one_indexed > self.tabs.len() {
            return false;
        }
        self.activate(self.tabs[one_indexed - 1]);
        true
    }

    /// `Some(tab)` index in the positional order.
    pub fn position_of(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|&t| t == tab)
    }

    /// §H6 — read the tab id at zero-based positional `index` without
    /// mutating active state or the MRU stack. Used by the Ctrl+Tab
    /// overlay's preview path: the highlighted row may be shown in the
    /// pane while Ctrl is still held, but MRU only updates on commit
    /// (Ctrl release / Enter).
    #[must_use]
    pub fn peek_positional(&self, index: usize) -> Option<TabId> {
        self.tabs.get(index).copied()
    }

    /// §H6 — set `active` without touching the MRU stack. Used by the
    /// Ctrl+Tab overlay's preview path; the chord-commit path calls
    /// `activate` instead so the chosen tab also bubbles to the head
    /// of `mru`. Returns `true` when `tab` is part of this group.
    pub fn set_active_for_preview(&mut self, tab: TabId) -> bool {
        if !self.tabs.contains(&tab) {
            return false;
        }
        self.active = tab;
        true
    }
}

/// Recently-closed tab record (in-memory; persisted entries live in the
/// `trash` table so this list survives only within a session).
#[derive(Debug, Clone)]
pub struct ClosedTab {
    /// Buffer the tab pointed at.
    pub buffer_id: BufferId,
    /// Resolved label at close time.
    pub label: String,
    /// Wall-clock millis at close.
    pub closed_at_ms: u64,
    /// Pane that hosted the tab when it was closed. `None` for legacy
    /// records decoded from JSON written before the field existed.
    /// `reopen_closed_tab` routes the reopened tab back to this pane
    /// when it still exists in the tree; otherwise it falls back to
    /// the parent-split anchor below.
    pub origin_pane: Option<PaneId>,
    /// Axis of the split that directly contained `origin_pane` at
    /// close time. Used by reopen to re-split the surviving sibling
    /// when `origin_pane` has been collapsed out of the tree.
    pub parent_split_axis: Option<SplitAxis>,
    /// First leaf among the closed pane's siblings at close time. When
    /// the origin pane has collapsed but this sibling is still a leaf
    /// in the tree, reopen splits that sibling along `parent_split_axis`
    /// so the reopened tab lands in roughly its original tree position.
    pub parent_sibling_leaf: Option<PaneId>,
}

/// The full pane tree owned by a window.
#[derive(Debug, Clone)]
pub struct PaneTree {
    /// Tree shape — internal `Split` nodes referencing `Leaf(PaneId)`.
    pub root: PaneNode,
    /// Group payload by id.
    pub groups: HashMap<PaneId, Group>,
    /// Tab payload by id.
    pub tabs: HashMap<TabId, Tab>,
    /// Currently focused leaf — `tabs[groups[focused].active]` is the
    /// active editor in the window.
    pub focused: PaneId,
    /// Recently-closed tab ring buffer (most recent first).
    pub recently_closed: Vec<ClosedTab>,
    /// Pane that is "maximized" within the window. When `Some`, render
    /// only that group; tree shape is preserved for unmaximize.
    pub maximized: Option<PaneId>,
}

impl PaneTree {
    /// Build a tree with a single tab and group.
    pub fn singleton(buffer_id: BufferId, created_at_ms: u64) -> Self {
        let tab = Tab::new(buffer_id, created_at_ms);
        let tab_id = tab.id;
        let group = Group::singleton(tab_id);
        let pane = group.id;
        let mut groups = HashMap::new();
        groups.insert(pane, group);
        let mut tabs = HashMap::new();
        tabs.insert(tab_id, tab);
        Self {
            root: PaneNode::Leaf(pane),
            groups,
            tabs,
            focused: pane,
            recently_closed: Vec::new(),
            maximized: None,
        }
    }

    /// Active buffer id (focused group, focused tab). Returns
    /// [`BufferId::nil`] when the focused tab is a non-buffer kind
    /// (e.g. [`TabKind::BufferHistory`]); prefer
    /// [`Self::active_buffer_opt`] for code that needs to branch on
    /// kind. The Option-returning accessor + per-tab predicates live
    /// in `pane_tree_kind.rs`.
    pub fn active_buffer(&self) -> BufferId {
        let group = &self.groups[&self.focused];
        self.tabs[&group.active].buffer_id
    }

    /// Move focus to `pane`. No-op if `pane` is unknown.
    pub fn focus(&mut self, pane: PaneId) {
        if self.groups.contains_key(&pane) {
            self.focused = pane;
        }
    }

    /// Open a new tab in the focused group with `buffer_id`. Returns the
    /// new `TabId`.
    pub fn open_tab_in_focused(&mut self, buffer_id: BufferId, created_at_ms: u64) -> TabId {
        let id = self.insert_fresh_buffer_tab(buffer_id, created_at_ms);
        if let Some(g) = self.groups.get_mut(&self.focused) {
            g.push_tab(id, true);
        }
        id
    }

    /// Pin `label` as the explicit `label_override` on every tab whose
    /// `buffer_id` matches. Used for synthetic surfaces (e.g. the
    /// metrics dashboard buffer) whose rope stays empty and would
    /// otherwise resolve to `"Untitled"` in the tab strip. Idempotent;
    /// returns the number of tabs relabeled.
    pub(crate) fn set_label_override_for_buffer(
        &mut self,
        buffer_id: BufferId,
        label: &str,
    ) -> usize {
        let mut relabeled = 0;
        for tab in self.tabs.values_mut() {
            if tab.buffer_id == buffer_id {
                tab.label_override = Some(label.to_string());
                relabeled += 1;
            }
        }
        relabeled
    }

    /// Insert a buffer tab with an id that is unused in this tree.
    pub(crate) fn insert_fresh_buffer_tab(
        &mut self,
        buffer_id: BufferId,
        created_at_ms: u64,
    ) -> TabId {
        let id = self.fresh_unused_tab_id();
        let tab = Tab::with_id(id, buffer_id, created_at_ms);
        self.tabs.insert(id, tab);
        id
    }

    /// Allocate a pane id that is unused in this tree.
    pub(crate) fn fresh_unused_pane_id(&self) -> PaneId {
        let leaf_ids = self.root.leaf_ids();
        loop {
            let id = PaneId::fresh();
            if !self.groups.contains_key(&id) && !leaf_ids.contains(&id) {
                return id;
            }
        }
    }

    /// Allocate a tab id that is unused in this tree.
    pub(crate) fn fresh_unused_tab_id(&self) -> TabId {
        loop {
            let id = TabId::fresh();
            if !self.tabs.contains_key(&id) {
                return id;
            }
        }
    }
}

#[cfg(test)]
#[path = "pane_tree/tests.rs"]
mod tests;
