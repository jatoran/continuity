//! δ.4 — buffer recovery helper.
//!
//! Rebuilds a [`Buffer`] from its persisted latest snapshot + trailing
//! edit log. Shared between `app::main` (startup recovery) and
//! `ui::Window` (the previous-buffer browser's "open closed buffer"
//! path).
//!
//! Halts replay at the first checksum mismatch (spec §4): returns
//! whatever has been validated so far, plus a [`RecoveryHalt`] that
//! describes the halt so the UI can banner the user.

use continuity_buffer::{Buffer, BufferId, Revision};

use crate::checksum::fnv1a_64_chunks;
use crate::codec::decode_op;
use crate::handle::PersistClient;
use crate::store::EditRow;
use crate::store::SnapshotRow;
use crate::Error;

/// Why replay halted before applying every persisted edit row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryHaltReason {
    /// The edit row's payload could not be decoded into an `EditOp`.
    /// Carries the decode error message.
    DecodeFailed(String),
    /// `Buffer::apply` returned an error on the decoded op. Carries the
    /// underlying buffer error message.
    ApplyFailed(String),
    /// The buffer's rope checksum after applying the decoded op did not
    /// match the row's `checksum_after`. Halt-on-mismatch protects
    /// against silently propagating a corrupt edit log past the first
    /// divergence.
    ChecksumMismatch {
        /// Checksum recomputed from the rope.
        computed: u64,
        /// Checksum recorded on the row.
        expected: u64,
    },
}

/// Description of why and where replay halted for a single buffer.
/// Emitted alongside the rebuilt [`Buffer`] (which carries state at
/// the last validated revision) so callers can banner the user.
#[derive(Clone, Debug)]
pub struct RecoveryHalt {
    /// Buffer whose replay halted.
    pub buffer_id: BufferId,
    /// `seq` of the row that triggered the halt (i.e. the row that
    /// failed to decode / apply / verify).
    pub halted_at_seq: u64,
    /// Revision the buffer was at immediately before the halt — the
    /// latest state the user can keep editing from.
    pub last_valid_revision: Revision,
    /// What failed.
    pub reason: RecoveryHaltReason,
}

/// Output of [`recover_buffer`]: the rebuilt buffer + the seq value
/// the next [`crate::PersistMessage::AppendEdit`] should carry. Pair
/// these with the wall-clock at recovery time to call
/// `EditorHandle::adopt_buffer`.
pub struct RecoveredBuffer {
    /// Fully-rebuilt buffer at the latest replayable revision.
    pub buffer: Buffer,
    /// `seq` value to assign to the next [`crate::PersistMessage::AppendEdit`].
    pub next_seq: u64,
    /// `Some` when replay halted before consuming every available edit
    /// row. The buffer above is still usable — it just stops at the
    /// last valid revision.
    pub halt: Option<RecoveryHalt>,
}

/// Synchronously recover `buffer_id` from the persistence layer.
/// Returns `Ok(None)` when the buffer has no snapshot (never persisted).
///
/// # Errors
///
/// Propagates any [`Error`] from the underlying queries.
pub fn recover_buffer(
    persist: &PersistClient,
    buffer_id: BufferId,
) -> Result<Option<RecoveredBuffer>, Error> {
    let Some(snap) = persist.load_latest_snapshot(buffer_id)? else {
        return Ok(None);
    };
    let edits = persist.edits_since(buffer_id, snap.revision)?;
    let next_seq = persist.next_seq(buffer_id)?;
    let file = persist.load_buffer_file(buffer_id)?;
    let (buffer, halt) = rebuild_buffer_with_halt(buffer_id, &snap, edits, file);
    Ok(Some(RecoveredBuffer {
        buffer,
        next_seq,
        halt,
    }))
}

/// Replay the edit log on top of the snapshot, halting at the first
/// checksum mismatch (spec §4). The halt-info-less wrapper retained
/// for callers that don't care about why replay stopped.
pub fn rebuild_buffer(
    id: BufferId,
    snap: &SnapshotRow,
    edits: Vec<EditRow>,
    file: Option<continuity_buffer::FileAssociation>,
) -> Buffer {
    rebuild_buffer_with_halt(id, snap, edits, file).0
}

/// Replay the edit log and return both the rebuilt buffer and (if
/// replay halted) the [`RecoveryHalt`] describing the halt cause.
pub fn rebuild_buffer_with_halt(
    id: BufferId,
    snap: &SnapshotRow,
    edits: Vec<EditRow>,
    file: Option<continuity_buffer::FileAssociation>,
) -> (Buffer, Option<RecoveryHalt>) {
    let mut buf = Buffer::from_parts_with_file(id, &snap.content, snap.revision, file);
    let mut halt: Option<RecoveryHalt> = None;
    for row in edits {
        let last_valid_revision = buf.revision();
        let op = match decode_op(&row) {
            Ok(op) => op,
            Err(e) => {
                halt = Some(RecoveryHalt {
                    buffer_id: id,
                    halted_at_seq: row.seq,
                    last_valid_revision,
                    reason: RecoveryHaltReason::DecodeFailed(e.to_string()),
                });
                break;
            }
        };
        if let Err(e) = buf.apply(&op) {
            halt = Some(RecoveryHalt {
                buffer_id: id,
                halted_at_seq: row.seq,
                last_valid_revision,
                reason: RecoveryHaltReason::ApplyFailed(e.to_string()),
            });
            break;
        }
        let checksum = fnv1a_64_chunks(buf.rope().chunks().map(str::as_bytes));
        if checksum != row.checksum_after {
            halt = Some(RecoveryHalt {
                buffer_id: id,
                halted_at_seq: row.seq,
                last_valid_revision,
                reason: RecoveryHaltReason::ChecksumMismatch {
                    computed: checksum,
                    expected: row.checksum_after,
                },
            });
            break;
        }
    }
    (buf, halt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::PersistHandle;

    #[test]
    fn recover_buffer_returns_none_for_never_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let h = PersistHandle::spawn(&dir.path().join("r.db")).unwrap();
        let c = h.client();
        let out = recover_buffer(&c, BufferId::new()).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn recover_buffer_round_trips_snapshot_content() {
        let dir = tempfile::tempdir().unwrap();
        let h = PersistHandle::spawn(&dir.path().join("r.db")).unwrap();
        let c = h.client();
        let original = Buffer::from_text("# Title\nbody\n");
        c.save_snapshot_blocking(original.id(), original.snapshot())
            .unwrap();
        let recovered = recover_buffer(&c, original.id()).unwrap().unwrap();
        assert_eq!(recovered.buffer.id(), original.id());
        let text: String = recovered.buffer.rope().chunks().collect();
        assert_eq!(text, "# Title\nbody\n");
        assert!(recovered.halt.is_none());
    }

    #[test]
    fn rebuild_buffer_with_halt_reports_checksum_mismatch() {
        use crate::store::EditRow;
        use continuity_buffer::{BufferId, Revision};

        // Build a snapshot at revision 0 with content "ab".
        let buffer_id = BufferId::new();
        let snap = SnapshotRow {
            id: Some(1),
            buffer_id,
            revision: Revision(0),
            content: "ab".to_string(),
            byte_len: 2,
            line_count: 1,
            checksum: 0,
            label: None,
            created_at_ms: 0,
        };

        // Hand-craft an insert("c") row that bumps revision 0 → 1 with a
        // deliberately wrong `checksum_after` so replay halts.
        let row = EditRow {
            buffer_id,
            seq: 1,
            revision: Revision(1),
            ts_ms: 0,
            op_kind: "insert".into(),
            range_start_line: Some(0),
            range_start_byte: Some(2),
            range_end_line: None,
            range_end_byte: None,
            inserted_text: Some("c".into()),
            removed_text: None,
            selections_before_json: None,
            selections_after_json: None,
            undo_group_id: None,
            checksum_after: 0xDEAD_BEEF_DEAD_BEEF, // intentionally wrong
        };

        let (rebuilt, halt) = rebuild_buffer_with_halt(buffer_id, &snap, vec![row], None);
        // The op applied successfully; only the checksum verification
        // failed, so the rope reflects the applied state but the halt
        // is recorded.
        let text: String = rebuilt.rope().chunks().collect();
        assert_eq!(text, "abc");
        let halt = halt.expect("checksum mismatch halt");
        assert_eq!(halt.buffer_id, buffer_id);
        assert_eq!(halt.halted_at_seq, 1);
        assert_eq!(halt.last_valid_revision, Revision(0));
        assert!(matches!(
            halt.reason,
            RecoveryHaltReason::ChecksumMismatch { .. }
        ));
    }

    #[test]
    fn rebuild_buffer_with_halt_is_none_on_clean_replay() {
        let buffer_id = BufferId::new();
        let snap = SnapshotRow {
            id: Some(1),
            buffer_id,
            revision: continuity_buffer::Revision(0),
            content: "hello".to_string(),
            byte_len: 5,
            line_count: 1,
            checksum: 0,
            label: None,
            created_at_ms: 0,
        };
        let (rebuilt, halt) = rebuild_buffer_with_halt(buffer_id, &snap, Vec::new(), None);
        assert_eq!(rebuilt.id(), buffer_id);
        assert!(halt.is_none());
    }
}
