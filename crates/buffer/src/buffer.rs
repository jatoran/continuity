//! The `Buffer` aggregate.
//!
//! Owned exclusively by the editor core thread. Carries id, rope, revision,
//! selections, and the undo tree.

mod selection_transform;

use std::sync::Arc;

use continuity_text::{EditOp, Position, Selection};
use ropey::Rope;

use crate::checksum;
use crate::selection_clamp::clamp_selection_to_rope;
use crate::{BufferId, Error, FileAssociation, Revision, RopeSnapshot, UndoTree};

use selection_transform::SelectionTransform;

/// A single editable text buffer.
///
/// **Thread ownership**: only the editor core thread mutates a `Buffer`.
/// All other threads receive [`RopeSnapshot`]s instead.
#[derive(Debug)]
pub struct Buffer {
    id: BufferId,
    rope: Rope,
    revision: Revision,
    selections: Vec<Selection>,
    undo: UndoTree,
    file: Option<FileAssociation>,
    /// Synthetic buffers are never written to persist (no `buffers` row,
    /// no `buffer_edits`, no `buffer_snapshots`). Used for transient
    /// read-only surfaces like the tutorial tab — the rope lives only
    /// in memory for the lifetime of the process.
    ///
    /// Construct via [`Buffer::synthetic_read_only`]. Default = `false`.
    synthetic: bool,
    /// Read-only buffers reject [`EditOp`] / [`crate::SelectionEdit`]
    /// application in the core thread before reaching the rope. Pairs
    /// naturally with `synthetic` for static content (tutorial, release
    /// notes) but is independent — a persisted buffer could be marked
    /// read-only without being synthetic.
    ///
    /// Default = `false`.
    read_only: bool,
    /// FNV-1a 64-bit hash of `rope`'s bytes, kept in sync with each
    /// [`Self::apply`] via incremental mix/unmix in
    /// [`crate::checksum::update_for_edit`]. Persisted on every edit
    /// row as `checksum_after`; full-walk verification reseats it
    /// periodically (every [`crate::checksum::CHECKSUM_VERIFY_INTERVAL`]
    /// edits, plus at snapshot boundaries) via
    /// [`Self::verify_running_checksum`].
    running_checksum: u64,
    /// Number of [`Self::apply`] calls since the last full-walk
    /// verification of [`Self::running_checksum`]. The core thread
    /// reads this to decide whether to take the verify path on the
    /// next persisted edit.
    edits_since_verify: u32,
}

impl Buffer {
    /// An empty buffer with a fresh id.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            id: BufferId::new(),
            rope: Rope::new(),
            revision: Revision::INITIAL,
            selections: vec![Selection::caret_at(Position::ZERO)],
            undo: UndoTree::new(),
            file: None,
            synthetic: false,
            read_only: false,
            running_checksum: checksum::FNV_OFFSET_BASIS,
            edits_since_verify: 0,
        }
    }

    /// A buffer pre-populated with `text`.
    pub fn from_text(text: &str) -> Self {
        let rope = Rope::from_str(text);
        let running_checksum = checksum::full_walk_rope(&rope);
        Self {
            id: BufferId::new(),
            rope,
            revision: Revision::INITIAL,
            selections: vec![Selection::caret_at(Position::ZERO)],
            undo: UndoTree::new(),
            file: None,
            synthetic: false,
            read_only: false,
            running_checksum,
            edits_since_verify: 0,
        }
    }

    /// A synthetic, read-only buffer pre-populated with `text`.
    ///
    /// "Synthetic" means the buffer is never persisted: no row in the
    /// `buffers` table, no `buffer_edits`, no `buffer_snapshots`, never
    /// surfaced in FTS5 or `find-in-all`. The rope lives only in memory
    /// for the lifetime of the process. "Read-only" means the core
    /// thread rejects any [`EditOp`] or [`crate::SelectionEdit`] before
    /// it reaches the rope.
    ///
    /// Used for transient documentation surfaces such as the tutorial
    /// tab.
    #[must_use]
    pub fn synthetic_read_only(text: &str) -> Self {
        let mut b = Self::from_text(text);
        b.synthetic = true;
        b.read_only = true;
        b
    }

    /// Reconstruct a buffer with a specific id and revision (for persistence
    /// recovery). The `revision` should be the revision the snapshot was
    /// taken at; subsequent [`Self::apply`] calls advance it normally.
    #[must_use]
    pub fn from_parts(id: BufferId, text: &str, revision: Revision) -> Self {
        let rope = Rope::from_str(text);
        let running_checksum = checksum::full_walk_rope(&rope);
        Self {
            id,
            rope,
            revision,
            selections: vec![Selection::caret_at(Position::ZERO)],
            undo: UndoTree::new(),
            file: None,
            synthetic: false,
            read_only: false,
            running_checksum,
            edits_since_verify: 0,
        }
    }

    /// Reconstruct a buffer with a specific id, revision, and file
    /// association.
    #[must_use]
    pub fn from_parts_with_file(
        id: BufferId,
        text: &str,
        revision: Revision,
        file: Option<FileAssociation>,
    ) -> Self {
        let mut buffer = Self::from_parts(id, text, revision);
        buffer.file = file;
        buffer
    }

    /// `true` when this buffer is synthetic (never persisted).
    ///
    /// Persist clients check this before issuing any write; the core
    /// thread skips `persist.touch_buffer` / `persist.upsert_buffer` /
    /// `persist.save_snapshot_async` / `persist.write_edit` for
    /// synthetic buffers.
    #[must_use]
    pub fn is_synthetic(&self) -> bool {
        self.synthetic
    }

    /// `true` when this buffer rejects edit application.
    ///
    /// The core thread checks this before dispatching to
    /// `apply_one_edit` / `apply_selection_edit` and returns
    /// [`Error::ReadOnly`] without touching the rope. Undo / redo
    /// operations also no-op on read-only buffers.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The buffer's stable id.
    #[must_use]
    pub fn id(&self) -> BufferId {
        self.id
    }

    /// The current revision number.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Borrow the rope.
    #[must_use]
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Borrow the selections.
    #[must_use]
    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// Replace the selection set.
    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        self.selections = if selections.is_empty() {
            vec![Selection::caret_at(Position::ZERO)]
        } else {
            selections
                .into_iter()
                .map(|selection| clamp_selection_to_rope(&self.rope, selection))
                .collect()
        };
    }

    /// Borrow the undo tree.
    #[must_use]
    pub fn undo_tree(&self) -> &UndoTree {
        &self.undo
    }

    /// Mutable borrow of the undo tree. Used by the editor core thread to
    /// insert groups, append records, and step the current pointer along
    /// the undo / redo path.
    pub fn undo_tree_mut(&mut self) -> &mut UndoTree {
        &mut self.undo
    }

    /// Borrow the optional filesystem association.
    #[must_use]
    pub fn file_association(&self) -> Option<&FileAssociation> {
        self.file.as_ref()
    }

    /// Replace the filesystem association.
    pub fn set_file_association(&mut self, file: Option<FileAssociation>) {
        self.file = file;
    }

    /// Capture the substring `op` would remove from the current rope, if
    /// any. Returns an empty `String` for [`EditOp::Insert`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Text`] if the range falls outside the rope.
    pub fn capture_removed_text(&self, op: &EditOp) -> Result<String, Error> {
        match op {
            EditOp::Insert { .. } => Ok(String::new()),
            EditOp::Delete { range } | EditOp::Replace { range, .. } => {
                let start = range.start.to_byte_offset(&self.rope)?;
                let end = range.end.to_byte_offset(&self.rope)?;
                let lo = start.min(end);
                let hi = start.max(end);
                Ok(self.rope.byte_slice(lo..hi).to_string())
            }
        }
    }

    /// Take a cheap, send-able snapshot of the current rope + revision.
    #[must_use]
    pub fn snapshot(&self) -> RopeSnapshot {
        RopeSnapshot::new(Arc::new(self.rope.clone()), self.revision)
    }

    /// First non-empty trimmed line of the rope, clipped to `max_chars`. An
    /// empty buffer (or a buffer of pure whitespace) returns `None`.
    ///
    /// Used for buffer-switcher labels and the `windows.tabs` table when no
    /// user-set title exists.
    #[must_use]
    pub fn title(&self, max_chars: usize) -> Option<String> {
        derive_title(&self.rope, max_chars)
    }

    /// FNV-1a 64-bit hash of the rope's bytes, maintained incrementally
    /// across [`Self::apply`] calls. The core thread persists this as
    /// `buffer_edits.checksum_after`; a divergence from a full-walk
    /// recomputation is surfaced through `event:checksum_drift`. See
    /// [`crate::checksum`].
    #[must_use]
    pub fn running_checksum(&self) -> u64 {
        self.running_checksum
    }

    /// Number of [`Self::apply`] calls since the running checksum was
    /// last cross-checked against a full-walk recomputation.
    #[must_use]
    pub fn edits_since_verify(&self) -> u32 {
        self.edits_since_verify
    }

    /// Re-run a full-walk FNV-1a over the current rope and reseat the
    /// running checksum to the freshly-computed value. Returns
    /// `(observed_before, computed)` — `observed_before` is what the
    /// running counter held before reseating; equality means no drift.
    /// Also clears [`Self::edits_since_verify`] so the next verification
    /// is again `CHECKSUM_VERIFY_INTERVAL` edits away.
    pub fn verify_running_checksum(&mut self) -> (u64, u64) {
        let observed = self.running_checksum;
        let computed = checksum::full_walk_rope(&self.rope);
        self.running_checksum = computed;
        self.edits_since_verify = 0;
        (observed, computed)
    }

    /// Apply an [`EditOp`], advance the revision, and return the new revision.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Text`] if any position in `op` falls outside the rope.
    /// Returns [`Error::ReadOnly`] if the buffer is read-only (e.g. a
    /// synthetic tutorial buffer); the rope is not touched.
    pub fn apply(&mut self, op: &EditOp) -> Result<Revision, Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let transform = SelectionTransform::from_op(&self.rope, op)?;
        // Capture the bytes the op will remove (empty for `Insert`) so
        // we can unmix them from the running checksum after the rope
        // has been mutated.
        let removed_text = self.capture_removed_text(op)?;
        let old_rope = self.rope.clone();
        match op {
            EditOp::Insert { at, text } => {
                let byte = at.to_byte_offset(&self.rope)?;
                let char_idx = self.rope.byte_to_char(byte);
                self.rope.insert(char_idx, text);
            }
            EditOp::Delete { range } => {
                let start_byte = range.start.to_byte_offset(&self.rope)?;
                let end_byte = range.end.to_byte_offset(&self.rope)?;
                let start_char = self.rope.byte_to_char(start_byte);
                let end_char = self.rope.byte_to_char(end_byte);
                self.rope.remove(start_char..end_char);
            }
            EditOp::Replace { range, text } => {
                let start_byte = range.start.to_byte_offset(&self.rope)?;
                let end_byte = range.end.to_byte_offset(&self.rope)?;
                let start_char = self.rope.byte_to_char(start_byte);
                let end_char = self.rope.byte_to_char(end_byte);
                self.rope.remove(start_char..end_char);
                self.rope.insert(start_char, text);
            }
        }
        // If the incremental update ever fails to resolve the op's
        // positions against the post-apply rope (none of the in-tree
        // ops do, but the signature returns `Result`), fall back to a
        // full walk so the running checksum stays bit-for-bit equal to
        // `hash(rope)` and recovery never sees a drifted edit row.
        self.running_checksum =
            match checksum::update_for_edit(self.running_checksum, &self.rope, op, &removed_text) {
                Ok(state) => state,
                Err(_) => checksum::full_walk_rope(&self.rope),
            };
        self.edits_since_verify = self.edits_since_verify.saturating_add(1);
        self.selections = transform.apply_all(&old_rope, &self.rope, &self.selections);
        self.revision = self.revision.next();
        Ok(self.revision)
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::empty()
    }
}

/// First non-empty trimmed line of `rope`, clipped to `max_chars`.
///
/// Returns `None` when the rope is empty or only whitespace. The result is
/// trimmed of leading + trailing whitespace; trailing ellipsis ("…") is
/// appended only when truncation actually drops characters.
#[must_use]
pub fn derive_title(rope: &Rope, max_chars: usize) -> Option<String> {
    for line_idx in 0..rope.len_lines() {
        let mut line: String = rope.line(line_idx).to_string();
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() <= max_chars {
            return Some(trimmed.to_string());
        }
        let mut s: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
        s.push('…');
        return Some(s);
    }
    None
}

#[cfg(test)]
mod tests {
    use continuity_text::{Position, Range};

    use super::*;

    #[test]
    fn empty_buffer_has_revision_zero() {
        let b = Buffer::empty();
        assert_eq!(b.revision(), Revision::INITIAL);
        assert_eq!(b.rope().len_bytes(), 0);
        assert_eq!(b.selections(), &[Selection::caret_at(Position::ZERO)]);
    }

    #[test]
    fn from_text_populates_rope() {
        let b = Buffer::from_text("hello");
        assert_eq!(b.rope().to_string(), "hello");
        assert_eq!(b.revision(), Revision::INITIAL);
    }

    #[test]
    fn insert_advances_revision_and_changes_rope() {
        let mut b = Buffer::from_text("hello");
        let r = b
            .apply(&EditOp::insert(Position::new(0, 5), " world"))
            .unwrap();
        assert_eq!(r, Revision(1));
        assert_eq!(b.rope().to_string(), "hello world");
    }

    #[test]
    fn delete_removes_range() {
        let mut b = Buffer::from_text("hello world");
        let range = Range::new(Position::new(0, 5), Position::new(0, 11));
        b.apply(&EditOp::delete(range)).unwrap();
        assert_eq!(b.rope().to_string(), "hello");
    }

    #[test]
    fn replace_swaps_range() {
        let mut b = Buffer::from_text("hello world");
        let range = Range::new(Position::new(0, 6), Position::new(0, 11));
        b.apply(&EditOp::replace(range, "rust")).unwrap();
        assert_eq!(b.rope().to_string(), "hello rust");
    }

    #[test]
    fn snapshot_carries_revision() {
        let mut b = Buffer::from_text("");
        b.apply(&EditOp::insert(Position::ZERO, "x")).unwrap();
        let s = b.snapshot();
        assert_eq!(s.revision(), Revision(1));
        assert_eq!(s.rope().to_string(), "x");
    }

    #[test]
    fn snapshot_does_not_observe_later_edits() {
        let mut b = Buffer::from_text("a");
        let s = b.snapshot();
        b.apply(&EditOp::insert(Position::new(0, 1), "b")).unwrap();
        assert_eq!(s.rope().to_string(), "a");
        assert_eq!(b.rope().to_string(), "ab");
    }

    #[test]
    fn from_parts_preserves_id_and_revision() {
        let id = BufferId::new();
        let b = Buffer::from_parts(id, "hello", Revision(7));
        assert_eq!(b.id(), id);
        assert_eq!(b.revision(), Revision(7));
        assert_eq!(b.rope().to_string(), "hello");
        assert_eq!(b.selections(), &[Selection::caret_at(Position::ZERO)]);
    }

    #[test]
    fn set_selections_keeps_at_least_one_caret() {
        let mut b = Buffer::from_text("hello");
        b.set_selections(Vec::new());
        assert_eq!(b.selections(), &[Selection::caret_at(Position::ZERO)]);
    }

    #[test]
    fn set_selections_clamps_positions_to_rope() {
        let mut b = Buffer::from_text("hello\nworld");
        b.set_selections(vec![Selection::caret_at(Position::new(99, 12))]);
        assert_eq!(b.selections(), &[Selection::caret_at(Position::new(1, 5))]);

        b.set_selections(vec![Selection::caret_at(Position::new(0, 99))]);
        assert_eq!(b.selections(), &[Selection::caret_at(Position::new(0, 5))]);
    }

    #[test]
    fn from_parts_then_apply_advances_revision() {
        let id = BufferId::new();
        let mut b = Buffer::from_parts(id, "ab", Revision(3));
        let r = b.apply(&EditOp::insert(Position::new(0, 2), "c")).unwrap();
        assert_eq!(r, Revision(4));
        assert_eq!(b.rope().to_string(), "abc");
    }

    #[test]
    fn title_picks_first_nonempty_line() {
        let b = Buffer::from_text("\n\n  hello world  \nsecond\n");
        assert_eq!(b.title(80).as_deref(), Some("hello world"));
    }

    #[test]
    fn title_truncates_with_ellipsis() {
        let long = "a".repeat(50);
        let b = Buffer::from_text(&long);
        let title = b.title(10).unwrap();
        assert_eq!(title.chars().count(), 10);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn title_returns_none_for_empty() {
        let b = Buffer::empty();
        assert_eq!(b.title(80), None);
    }

    #[test]
    fn title_returns_none_for_whitespace_only() {
        let b = Buffer::from_text("\n\n   \n\t\n");
        assert_eq!(b.title(80), None);
    }

    #[test]
    fn out_of_bounds_edit_errors() {
        let mut b = Buffer::from_text("hi");
        let op = EditOp::insert(Position::new(99, 0), "x");
        assert!(b.apply(&op).is_err());
        assert_eq!(b.revision(), Revision::INITIAL);
    }

    #[test]
    fn default_buffer_is_not_synthetic_or_read_only() {
        let b = Buffer::empty();
        assert!(!b.is_synthetic());
        assert!(!b.is_read_only());
        let b = Buffer::from_text("hi");
        assert!(!b.is_synthetic());
        assert!(!b.is_read_only());
        let b = Buffer::from_parts(BufferId::new(), "hi", Revision::INITIAL);
        assert!(!b.is_synthetic());
        assert!(!b.is_read_only());
    }

    #[test]
    fn synthetic_read_only_carries_both_flags_and_text() {
        let b = Buffer::synthetic_read_only("# Tutorial\n\nbody");
        assert!(b.is_synthetic());
        assert!(b.is_read_only());
        assert_eq!(b.rope().to_string(), "# Tutorial\n\nbody");
    }

    #[test]
    fn read_only_buffer_rejects_edits_without_touching_rope() {
        let mut b = Buffer::synthetic_read_only("hello");
        let r = b.apply(&EditOp::insert(Position::new(0, 5), " world"));
        assert!(matches!(r, Err(Error::ReadOnly)));
        assert_eq!(b.rope().to_string(), "hello");
        assert_eq!(b.revision(), Revision::INITIAL);
    }

    #[test]
    fn running_checksum_initializes_from_initial_text() {
        let b = Buffer::from_text("hello world");
        assert_eq!(
            b.running_checksum(),
            crate::checksum::full_walk_rope(&Rope::from_str("hello world")),
        );
    }

    #[test]
    fn running_checksum_tracks_apply_chain() {
        let mut b = Buffer::from_text("abc");
        b.apply(&EditOp::insert(Position::new(0, 3), "def"))
            .unwrap();
        b.apply(&EditOp::delete(Range::new(
            Position::new(0, 1),
            Position::new(0, 4),
        )))
        .unwrap();
        b.apply(&EditOp::replace(
            Range::new(Position::new(0, 1), Position::new(0, 2)),
            "ZZ",
        ))
        .unwrap();
        assert_eq!(
            b.running_checksum(),
            crate::checksum::full_walk_rope(b.rope()),
        );
    }

    #[test]
    fn verify_running_checksum_reseats_and_clears_counter() {
        let mut b = Buffer::from_text("");
        for ch in ["a", "b", "c", "d"] {
            let len = b.rope().len_bytes() as u32;
            b.apply(&EditOp::insert(Position::new(0, len), ch)).unwrap();
        }
        assert_eq!(b.edits_since_verify(), 4);
        let (observed, computed) = b.verify_running_checksum();
        assert_eq!(observed, computed);
        assert_eq!(b.edits_since_verify(), 0);
    }
}
