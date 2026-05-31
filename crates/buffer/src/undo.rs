//! Per-buffer undo **tree**, persisted in `undo_groups` (and the edit log).
//!
//! Branches form when the user undoes a group and then makes a different
//! edit instead of redoing — the new edit becomes a sibling rather than
//! overwriting the redo branch (per spec §8).
//!
//! **Thread ownership**: only the editor core thread mutates an `UndoTree`;
//! it lives inside [`crate::Buffer`].

use ahash::AHashMap;
use continuity_text::{EditOp, Selection};

use crate::{Revision, UndoGroupId};

/// One atomic edit recorded in an undo group, alongside the inverse op the
/// undo path replays to revert it.
#[derive(Clone, Debug)]
pub struct EditRecord {
    /// The op that was applied.
    pub op: EditOp,
    /// The op that, applied to the *post-edit* rope, restores the
    /// *pre-edit* rope. Computed at record time when both the original
    /// removed text and the post-edit positions are still in scope.
    pub inverse_op: EditOp,
    /// Buffer revision before [`Self::op`] was applied.
    pub revision_before: Revision,
    /// Buffer revision after [`Self::op`] was applied.
    pub revision_after: Revision,
    /// Selections at the time of the edit, before mutation.
    pub selections_before: Vec<Selection>,
    /// Selections after mutation.
    pub selections_after: Vec<Selection>,
}

/// A coalesced unit of undo (typing in a single 500ms run, one paste, one
/// multi-cursor edit, one find-replace fanout, etc.).
#[derive(Clone, Debug)]
pub struct UndoGroup {
    /// Stable id (UUIDv7).
    pub id: UndoGroupId,
    /// The group's parent in the tree. `None` for the root group.
    pub parent: Option<UndoGroupId>,
    /// Edits in application order.
    pub ops: Vec<EditRecord>,
    /// Wall-clock millis of group creation.
    pub timestamp_ms: i64,
    /// The command that produced this group (for display in the picker and
    /// the persisted `undo_groups.command_name` column).
    pub command: String,
}

impl UndoGroup {
    /// True iff this group has at least one recorded edit.
    #[must_use]
    pub fn has_ops(&self) -> bool {
        !self.ops.is_empty()
    }
}

/// The full per-buffer undo history.
///
/// The tree is append-only (groups never disappear from `groups` once
/// recorded; the history-cap policy is implemented by snapshotting the
/// content and dropping covered edit rows at the persistence layer).
#[derive(Debug, Default, Clone)]
pub struct UndoTree {
    groups: Vec<UndoGroup>,
    by_id: AHashMap<UndoGroupId, usize>,
    /// Index of the group whose ops have been applied to the buffer; `None`
    /// when the buffer is at the pre-history state.
    current: Option<usize>,
}

impl UndoTree {
    /// An empty tree (current = pre-history).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an empty group rooted at `parent` (or at the current head if
    /// `parent` is `None` and a current group exists). Returns the group's
    /// stable id.
    ///
    /// Used both at edit time (when the core thread mints a new group) and
    /// at recovery time (when persisted rows are replayed).
    pub fn insert_group(
        &mut self,
        id: UndoGroupId,
        parent: Option<UndoGroupId>,
        timestamp_ms: i64,
        command: impl Into<String>,
    ) {
        let parent = parent.or_else(|| self.current_id());
        let group = UndoGroup {
            id,
            parent,
            ops: Vec::new(),
            timestamp_ms,
            command: command.into(),
        };
        let idx = self.groups.len();
        self.by_id.insert(id, idx);
        self.groups.push(group);
        self.current = Some(idx);
    }

    /// Append an edit record to the named group. Touches `current` to point
    /// at that group.
    ///
    /// # Panics
    ///
    /// Panics if `group_id` is unknown (caller-bug only — the core thread
    /// always inserts the group before recording into it).
    pub fn append_record(&mut self, group_id: UndoGroupId, record: EditRecord) {
        let idx = *self
            .by_id
            .get(&group_id)
            .expect("invariant: group recorded into without insert_group");
        self.groups[idx].ops.push(record);
        self.current = Some(idx);
    }

    /// True when no groups have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Number of groups in the tree.
    #[must_use]
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Number of [`UndoGroup`]s recorded. Alias for [`Self::len`],
    /// surfaced separately because the trace stream emits
    /// `undo_tree_groups=` and `undo_tree_records=` side by side.
    #[must_use]
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Total number of [`EditRecord`]s across every group. Bumped by
    /// every [`Self::append_record`] call; combined with
    /// [`Self::byte_size_estimate`] this lets the memory_breakdown line
    /// report bytes-per-record so Block 4.1 can prove its
    /// `SmallVec<[Selection; 1]>` rewrite shrank per-record overhead.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.groups.iter().map(|group| group.ops.len()).sum()
    }

    /// Estimated heap bytes retained by the undo tree.
    #[must_use]
    pub fn byte_size_estimate(&self) -> usize {
        self.groups.capacity() * std::mem::size_of::<UndoGroup>()
            + self.by_id.capacity()
                * (std::mem::size_of::<UndoGroupId>() + std::mem::size_of::<usize>())
            + self.groups.iter().map(undo_group_heap_bytes).sum::<usize>()
    }

    /// The currently-applied group, if any.
    #[must_use]
    pub fn current(&self) -> Option<&UndoGroup> {
        self.current.map(|i| &self.groups[i])
    }

    /// Id of the currently-applied group.
    #[must_use]
    pub fn current_id(&self) -> Option<UndoGroupId> {
        self.current().map(|g| g.id)
    }

    /// Borrow a group by id.
    #[must_use]
    pub fn get(&self, id: UndoGroupId) -> Option<&UndoGroup> {
        self.by_id.get(&id).map(|&i| &self.groups[i])
    }

    /// Borrow every group in insertion order. Used by the picker UI.
    #[must_use]
    pub fn groups(&self) -> &[UndoGroup] {
        &self.groups
    }

    /// Step the current pointer to `id`, asserting the group exists.
    ///
    /// # Panics
    ///
    /// Panics if `id` is unknown — the call sites compute targets only from
    /// existing groups, so this is a structural invariant.
    pub fn set_current(&mut self, id: Option<UndoGroupId>) {
        match id {
            None => self.current = None,
            Some(id) => {
                let idx = *self
                    .by_id
                    .get(&id)
                    .expect("invariant: set_current with unknown group id");
                self.current = Some(idx);
            }
        }
    }

    /// The group that an `editor.undo` from the current head would revert,
    /// i.e. the current group itself.
    #[must_use]
    pub fn group_to_undo(&self) -> Option<&UndoGroup> {
        self.current()
    }

    /// The group that an `editor.redo` from the current head would re-apply
    /// — the most-recently-recorded direct child of the current head (or of
    /// the pre-history root if `current` is `None`).
    ///
    /// The "most recent" tie-breaker matches the user's most recent intent:
    /// if you undo and then redo, you redo the branch you just left.
    #[must_use]
    pub fn group_to_redo(&self) -> Option<&UndoGroup> {
        self.children(self.current_id()).into_iter().next_back()
    }

    /// An alternate redo target: a child of the current pointer that is not
    /// the one [`Self::group_to_redo`] would pick. Returns `None` when the
    /// current pointer has fewer than two children.
    ///
    /// Used to step through fork branches after the user has undone past
    /// the divergence point.
    #[must_use]
    pub fn group_to_redo_alternate(&self) -> Option<&UndoGroup> {
        let kids = self.children(self.current_id());
        if kids.len() < 2 {
            return None;
        }
        let primary = kids.last()?.id;
        kids.into_iter().find(|g| g.id != primary)
    }

    /// Direct children of `parent` in insertion order (oldest first).
    /// `parent = None` returns the root-level branches.
    #[must_use]
    pub fn children(&self, parent: Option<UndoGroupId>) -> Vec<&UndoGroup> {
        self.groups.iter().filter(|g| g.parent == parent).collect()
    }
}

// Reserved trace event: `event:undo_tree_trim`. Emitted from the
// trimming code path introduced by Block 4.2 of the memory
// optimization plan (Emacs-style three-tier byte-budget undo cap).
// The event is not produced anywhere yet — when the trimmer lands it
// should emit `event:undo_tree_trim buffer=<id> tier=<soft|hard|panic>
// dropped_groups=<n> dropped_records=<n> freed_bytes=<n>` so xtask
// perf-report can correlate undo-tree shrinkage with the budget
// trigger.

fn undo_group_heap_bytes(group: &UndoGroup) -> usize {
    group.command.capacity()
        + group.ops.capacity() * std::mem::size_of::<EditRecord>()
        + group.ops.iter().map(edit_record_heap_bytes).sum::<usize>()
}

fn edit_record_heap_bytes(record: &EditRecord) -> usize {
    edit_op_heap_bytes(&record.op)
        + edit_op_heap_bytes(&record.inverse_op)
        + record.selections_before.capacity() * std::mem::size_of::<Selection>()
        + record.selections_after.capacity() * std::mem::size_of::<Selection>()
}

fn edit_op_heap_bytes(op: &EditOp) -> usize {
    match op {
        EditOp::Insert { text, .. } | EditOp::Replace { text, .. } => text.capacity(),
        EditOp::Delete { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use continuity_text::{Position, Range};

    use super::*;

    fn record(rev: u64) -> EditRecord {
        EditRecord {
            op: EditOp::insert(Position::ZERO, "x"),
            inverse_op: EditOp::delete(Range::new(Position::ZERO, Position::new(0, 1))),
            revision_before: Revision(rev - 1),
            revision_after: Revision(rev),
            selections_before: vec![],
            selections_after: vec![],
        }
    }

    #[test]
    fn empty_tree_has_no_current() {
        let t = UndoTree::new();
        assert!(t.is_empty());
        assert!(t.current().is_none());
    }

    #[test]
    fn inserting_groups_links_parent_chain() {
        let mut t = UndoTree::new();
        let a = UndoGroupId::new();
        t.insert_group(a, None, 0, "editor.insert_char");
        t.append_record(a, record(1));
        let b = UndoGroupId::new();
        t.insert_group(b, None, 1, "editor.insert_char");
        t.append_record(b, record(2));
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(b).unwrap().parent, Some(a));
        assert_eq!(t.current_id(), Some(b));
    }

    #[test]
    fn group_to_undo_is_current() {
        let mut t = UndoTree::new();
        let a = UndoGroupId::new();
        t.insert_group(a, None, 0, "x");
        t.append_record(a, record(1));
        assert_eq!(t.group_to_undo().map(|g| g.id), Some(a));
    }

    #[test]
    fn group_to_redo_picks_most_recent_child() {
        let mut t = UndoTree::new();
        let a = UndoGroupId::new();
        t.insert_group(a, None, 0, "x");
        t.append_record(a, record(1));
        let b = UndoGroupId::new();
        t.insert_group(b, Some(a), 1, "y");
        t.append_record(b, record(2));
        // Branch after undoing back to a:
        t.set_current(Some(a));
        let c = UndoGroupId::new();
        t.insert_group(c, Some(a), 2, "z");
        t.append_record(c, record(3));
        t.set_current(Some(a));
        assert_eq!(t.group_to_redo().map(|g| g.id), Some(c));
    }

    #[test]
    fn redo_alternate_picks_other_child() {
        let mut t = UndoTree::new();
        let a = UndoGroupId::new();
        t.insert_group(a, None, 0, "x");
        t.append_record(a, record(1));
        let b = UndoGroupId::new();
        t.insert_group(b, Some(a), 1, "y");
        t.append_record(b, record(2));
        t.set_current(Some(a));
        let c = UndoGroupId::new();
        t.insert_group(c, Some(a), 2, "z");
        t.append_record(c, record(3));
        t.set_current(Some(a));
        // From parent `a`, the most-recent child is `c`; the alternate is `b`.
        assert_eq!(t.group_to_redo().map(|g| g.id), Some(c));
        assert_eq!(t.group_to_redo_alternate().map(|g| g.id), Some(b));
    }
}
