//! Pure replay of host-recorded change batches.

use continuity_buffer::{Buffer, BufferId, Revision};
use continuity_text::Selection;

use crate::{ChangeBatch, Error};

/// Reconstructed state after applying a host event log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayedState {
    /// Replayed buffer id.
    pub buffer_id: BufferId,
    /// Final UTF-8 text.
    pub text: String,
    /// Final revision.
    pub revision: Revision,
    /// Final selection set.
    pub selections: Vec<Selection>,
    /// Final running checksum.
    pub checksum: u64,
}

/// Replay ordered batches without creating an engine host, database, or file.
///
/// This reconstructs text, selections, revisions, and checksums. Undo-tree
/// restoration remains a host/storage capability rather than an engine load
/// requirement.
pub fn replay_change_batches(
    buffer_id: BufferId,
    initial_text: &str,
    initial_revision: Revision,
    initial_selections: Vec<Selection>,
    batches: &[ChangeBatch],
) -> Result<ReplayedState, Error> {
    let mut buffer = Buffer::from_parts(buffer_id, initial_text, initial_revision);
    buffer.set_selections(initial_selections);
    for batch in batches {
        if batch.buffer_id != buffer_id {
            return Err(Error::InvalidChangeBatch("buffer id changed".into()));
        }
        if batch.revision_before != buffer.revision() {
            return Err(Error::InvalidChangeBatch(
                "batch revision chain has a gap".into(),
            ));
        }
        if batch.selections_before != buffer.selections() {
            return Err(Error::InvalidChangeBatch(
                "batch selection chain has a gap".into(),
            ));
        }
        for change in &batch.changes {
            if change.revision_before != buffer.revision() {
                return Err(Error::InvalidChangeBatch(
                    "operation revision chain has a gap".into(),
                ));
            }
            let revision = buffer.apply(&change.op)?;
            if revision != change.revision_after
                || buffer.running_checksum() != change.checksum_after
            {
                return Err(Error::InvalidChangeBatch(
                    "operation result does not match recorded revision/checksum".into(),
                ));
            }
        }
        buffer.set_selections(batch.selections_after.clone());
        if buffer.revision() != batch.revision_after
            || buffer.running_checksum() != batch.checksum_after
        {
            return Err(Error::InvalidChangeBatch(
                "batch result does not match recorded revision/checksum".into(),
            ));
        }
    }
    Ok(ReplayedState {
        buffer_id,
        text: buffer.rope().to_string(),
        revision: buffer.revision(),
        selections: buffer.selections().to_vec(),
        checksum: buffer.running_checksum(),
    })
}
