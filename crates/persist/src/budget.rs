//! Persistence-queue byte accounting (Phase 17 / spec §2 backpressure).
//!
//! The persistence thread does not impose backpressure on the editor core
//! by blocking: spec §2 calls for the *core* thread to coalesce adjacent
//! insert/delete/replace records per `(buffer, undo_group)` once the
//! unflushed queue grows past 8 MiB. This module owns the byte-cost
//! constants and the per-message size estimate that
//! [`crate::handle::PersistClient`] uses to drive that decision.
//!
//! Estimates are deliberately approximate — the real cost on disk after
//! zstd-compression is irrelevant here; we want a fast, monotonically
//! decreasing gauge of "memory the persist thread has accepted but not
//! finished writing".

use continuity_buffer::RopeSnapshot;

use crate::store::EditRow;

/// Spec §2 backpressure threshold. Once the persistence queue exceeds this
/// many unflushed bytes, the editor core thread is expected to coalesce
/// adjacent edits per `(buffer, undo_group)` instead of enqueueing more
/// work, so disk slowness never produces typing lag on the UI/core hot
/// path.
pub const OVERLOAD_THRESHOLD_BYTES: usize = 8 * 1024 * 1024;

/// Lower-bound byte cost of one `AppendEdit` message — covers per-message
/// struct overhead, undo-group ids, JSON selection blobs, and the
/// inserted/removed text payloads. Used to grow the `unflushed_bytes`
/// counter without paying for an exact size walk on the hot path.
#[must_use]
pub fn edit_row_byte_cost(row: &EditRow) -> usize {
    const FIXED_OVERHEAD: usize = 256;
    let inserted = row.inserted_text.as_deref().map(str::len).unwrap_or(0);
    let removed = row.removed_text.as_deref().map(str::len).unwrap_or(0);
    let sels_before = row
        .selections_before_json
        .as_deref()
        .map(str::len)
        .unwrap_or(0);
    let sels_after = row
        .selections_after_json
        .as_deref()
        .map(str::len)
        .unwrap_or(0);
    FIXED_OVERHEAD + inserted + removed + sels_before + sels_after
}

/// Approximate byte cost of one `SaveSnapshot` message: the in-memory rope
/// payload plus message overhead. Snapshots are large; coalescing them is
/// not interesting (a fresher snapshot supersedes an older one).
#[must_use]
pub fn snapshot_byte_cost(snapshot: &RopeSnapshot) -> usize {
    const FIXED_OVERHEAD: usize = 128;
    FIXED_OVERHEAD + snapshot.rope().len_bytes()
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;

    use super::*;

    #[test]
    fn snapshot_cost_tracks_rope_len() {
        let buf = Buffer::from_text(&"x".repeat(512));
        let cost = snapshot_byte_cost(&buf.snapshot());
        assert!((512..512 + 4096).contains(&cost));
    }

    #[test]
    fn overload_threshold_is_8_mib() {
        assert_eq!(OVERLOAD_THRESHOLD_BYTES, 8 * 1024 * 1024);
    }
}
