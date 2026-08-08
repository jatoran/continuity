//! Native SQLite adapter for storage-neutral engine change batches.

use ahash::AHashMap;
use continuity_buffer::BufferId;
use continuity_engine::ChangeBatch;
use continuity_persist::{encode_edit, PersistClient, UndoGroupRow};

/// Core-thread-owned edit-log sequence state and SQLite batch projection.
///
/// **Thread ownership**: the native core actor is the sole owner. The engine
/// never sees database row sequence numbers or persistence message types.
pub(crate) struct PersistenceBridge<'a> {
    persist: &'a PersistClient,
    next_sequence: AHashMap<BufferId, u64>,
}

impl<'a> PersistenceBridge<'a> {
    pub(crate) fn new(persist: &'a PersistClient) -> Self {
        Self {
            persist,
            next_sequence: AHashMap::new(),
        }
    }

    pub(crate) fn seed_next_sequence(&mut self, buffer_id: BufferId, next_sequence: u64) {
        self.next_sequence.insert(buffer_id, next_sequence);
    }

    pub(crate) fn persist_batch(&mut self, batch: &ChangeBatch) {
        if let Some(group) = &batch.new_undo_group {
            let _scope = crate::trace::Scope::new("core_write_undo_group_send");
            let _ = self.persist.write_undo_group(UndoGroupRow {
                id: group.id,
                buffer_id: batch.buffer_id,
                command_name: group.command.clone(),
                ts_ms: group.timestamp_ms,
                parent_group_id: group.parent,
            });
        }
        for change in &batch.changes {
            let sequence = self.next_sequence.entry(batch.buffer_id).or_insert(1);
            let row = {
                let _scope = crate::trace::Scope::new("core_encode_edit");
                encode_edit(
                    batch.buffer_id,
                    *sequence,
                    change.revision_after,
                    batch.timestamp_ms,
                    &change.op,
                    (!change.removed_text.is_empty()).then_some(change.removed_text.as_str()),
                    &change.selections_before,
                    &change.selections_after,
                    Some(batch.undo_group_id),
                    change.checksum_after,
                )
            };
            *sequence = sequence.saturating_add(1);
            let edit_sequence = crate::trace::current_edit_seq();
            let _scope = crate::trace::Scope::new("core_append_edit_send");
            if let Err(error) = self.persist.append_edit_with_seq(row, edit_sequence) {
                eprintln!("continuity-core: append_edit failed: {error}");
            }
        }
    }
}
