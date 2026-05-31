//! Snapshot policy: when to roll a new buffer snapshot.
//!
//! The core thread keeps a [`SnapshotTracker`] per buffer and calls
//! [`SnapshotTracker::record_edit`] every time it accepts an edit. Once the
//! tracker reports [`SnapshotTrigger::Threshold`], the core thread sends a
//! [`crate::EditorMessage`]-equivalent snapshot request to the persistence
//! thread and resets the tracker.
//!
//! Thresholds (per spec §4):
//! - 500 edits since last snapshot
//! - 256 KiB of cumulative byte-delta since last snapshot
//! - 60 s of activity since last snapshot

/// Thresholds that trigger an automatic snapshot.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotPolicy {
    /// Edits since the last snapshot.
    pub edits: u32,
    /// Cumulative bytes inserted-or-deleted since the last snapshot.
    pub bytes: usize,
    /// Wall-clock milliseconds since the last snapshot.
    pub interval_ms: i64,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self {
            edits: 500,
            bytes: 256 * 1024,
            interval_ms: 60_000,
        }
    }
}

/// What [`SnapshotTracker::record_edit`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTrigger {
    /// No snapshot needed.
    None,
    /// One of the policy thresholds has been crossed.
    Threshold,
}

/// Per-buffer counters used by [`SnapshotPolicy`].
///
/// Constructed via [`Self::starting_at`] when a buffer is opened or adopted
/// (the initial snapshot's timestamp).
#[derive(Debug, Clone, Copy)]
pub struct SnapshotTracker {
    edits_since: u32,
    bytes_since: usize,
    last_snapshot_at_ms: i64,
}

impl SnapshotTracker {
    /// Construct a tracker whose "last snapshot" was at `now_ms`.
    #[must_use]
    pub(crate) fn starting_at(now_ms: i64) -> Self {
        Self {
            edits_since: 0,
            bytes_since: 0,
            last_snapshot_at_ms: now_ms,
        }
    }

    /// Record an accepted edit and ask whether a new snapshot is due.
    ///
    /// `byte_delta` is the absolute number of bytes inserted plus deleted by
    /// the edit (callers can compute it from the [`continuity_text::EditOp`]
    /// directly — see [`edit_byte_delta`]).
    pub(crate) fn record_edit(
        &mut self,
        byte_delta: usize,
        now_ms: i64,
        policy: &SnapshotPolicy,
    ) -> SnapshotTrigger {
        self.edits_since = self.edits_since.saturating_add(1);
        self.bytes_since = self.bytes_since.saturating_add(byte_delta);
        let elapsed = now_ms.saturating_sub(self.last_snapshot_at_ms);
        if self.edits_since >= policy.edits
            || self.bytes_since >= policy.bytes
            || elapsed >= policy.interval_ms
        {
            SnapshotTrigger::Threshold
        } else {
            SnapshotTrigger::None
        }
    }

    /// Reset the counters, marking `now_ms` as the new "last snapshot" time.
    pub fn reset(&mut self, now_ms: i64) {
        self.edits_since = 0;
        self.bytes_since = 0;
        self.last_snapshot_at_ms = now_ms;
    }

    /// Edits accepted since the last snapshot. Useful in tests.
    #[must_use]
    pub fn edits_since(&self) -> u32 {
        self.edits_since
    }

    /// Bytes accumulated since the last snapshot. Useful in tests.
    #[must_use]
    pub(crate) fn bytes_since(&self) -> usize {
        self.bytes_since
    }
}

/// The number of bytes an [`continuity_text::EditOp`] inserts or deletes.
///
/// Inserted text length plus removed text length (for `Replace` both count).
/// Used to feed [`SnapshotTracker::record_edit`].
#[must_use]
pub fn edit_byte_delta(op: &continuity_text::EditOp, removed_text_len: usize) -> usize {
    match op {
        continuity_text::EditOp::Insert { text, .. } => text.len(),
        continuity_text::EditOp::Delete { .. } => removed_text_len,
        continuity_text::EditOp::Replace { text, .. } => text.len() + removed_text_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SnapshotPolicy {
        SnapshotPolicy {
            edits: 4,
            bytes: 100,
            interval_ms: 1_000,
        }
    }

    #[test]
    fn no_trigger_below_thresholds() {
        let mut t = SnapshotTracker::starting_at(0);
        let p = policy();
        assert_eq!(t.record_edit(10, 100, &p), SnapshotTrigger::None);
        assert_eq!(t.record_edit(10, 200, &p), SnapshotTrigger::None);
    }

    #[test]
    fn edit_count_threshold_fires() {
        let mut t = SnapshotTracker::starting_at(0);
        let p = policy();
        for _ in 0..3 {
            t.record_edit(1, 100, &p);
        }
        assert_eq!(t.record_edit(1, 100, &p), SnapshotTrigger::Threshold);
    }

    #[test]
    fn byte_threshold_fires() {
        let mut t = SnapshotTracker::starting_at(0);
        let p = policy();
        assert_eq!(t.record_edit(99, 100, &p), SnapshotTrigger::None);
        assert_eq!(t.record_edit(1, 100, &p), SnapshotTrigger::Threshold);
    }

    #[test]
    fn interval_threshold_fires() {
        let mut t = SnapshotTracker::starting_at(0);
        let p = policy();
        assert_eq!(t.record_edit(1, 999, &p), SnapshotTrigger::None);
        assert_eq!(t.record_edit(1, 1_000, &p), SnapshotTrigger::Threshold);
    }

    #[test]
    fn reset_zeros_counters() {
        let mut t = SnapshotTracker::starting_at(0);
        let p = policy();
        for _ in 0..4 {
            t.record_edit(50, 100, &p);
        }
        t.reset(500);
        assert_eq!(t.edits_since(), 0);
        assert_eq!(t.bytes_since(), 0);
        // After reset, a small edit doesn't fire.
        assert_eq!(t.record_edit(1, 600, &p), SnapshotTrigger::None);
    }

    #[test]
    fn default_policy_matches_spec() {
        let p = SnapshotPolicy::default();
        assert_eq!(p.edits, 500);
        assert_eq!(p.bytes, 262_144);
        assert_eq!(p.interval_ms, 60_000);
    }

    #[test]
    fn byte_delta_for_insert() {
        let op = continuity_text::EditOp::insert(continuity_text::Position::ZERO, "hello");
        assert_eq!(edit_byte_delta(&op, 0), 5);
    }

    #[test]
    fn byte_delta_for_delete_uses_removed_len() {
        let op = continuity_text::EditOp::delete(continuity_text::Range::empty(
            continuity_text::Position::ZERO,
        ));
        assert_eq!(edit_byte_delta(&op, 7), 7);
    }

    #[test]
    fn byte_delta_for_replace_sums_both() {
        let op = continuity_text::EditOp::replace(
            continuity_text::Range::empty(continuity_text::Position::ZERO),
            "world",
        );
        assert_eq!(edit_byte_delta(&op, 3), 8);
    }
}
