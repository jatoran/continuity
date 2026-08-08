//! Debounced continuous export for file buffers owned by an active vault.

use continuity_buffer::BufferId;

use crate::vault::PendingVaultAutosave;
use crate::window::Window;
use crate::window_file::FileBanner;

/// How long an autosave may stay `in_flight` before the watchdog assumes its
/// completion event was lost and clears it so the buffer can retry. Writes
/// normally acknowledge in single-digit milliseconds; 15 s is far beyond any
/// legitimate disk write yet short enough to self-heal within one session.
const STUCK_AUTOSAVE_MS: u64 = 15_000;

impl Window {
    pub(crate) fn schedule_vault_autosave(&mut self, buffer_id: BufferId) {
        if self.vault.suspended_autosaves.contains(&buffer_id) {
            return;
        }
        let Some(config) = self.vault.config() else {
            return;
        };
        if !config.save.autosave {
            return;
        }
        let delay_ms = config.save.delay_ms;
        let Some(snapshot) = self.editor.snapshot(buffer_id) else {
            return;
        };
        let revision = snapshot.rope_snapshot().revision().get();
        let Some(file) = snapshot.file else { return };
        if !self.vault.owns_file(&file.path) {
            return;
        }
        if self
            .vault
            .pending_autosaves
            .get(&buffer_id)
            .is_some_and(|pending| pending.revision == revision)
        {
            return;
        }
        self.vault.pending_autosaves.insert(
            buffer_id,
            PendingVaultAutosave {
                due_ms: self.now_ms().saturating_add(delay_ms),
                revision,
            },
        );
    }

    pub(crate) fn discover_dirty_vault_buffers(&mut self) {
        if !self.vault.is_active() {
            return;
        }
        let mut buffer_ids: Vec<_> = self.tree.tabs.values().map(|tab| tab.buffer_id).collect();
        buffer_ids.sort_by_key(|buffer_id| *buffer_id.as_uuid().as_bytes());
        buffer_ids.dedup();
        for buffer_id in buffer_ids {
            let Some(snapshot) = self.editor.snapshot(buffer_id) else {
                continue;
            };
            let Some(file) = snapshot.file.as_ref() else {
                continue;
            };
            if !self.vault.owns_file(&file.path) {
                continue;
            }
            let content_hash = continuity_persist::fnv1a_64(
                snapshot.rope_snapshot().rope().to_string().as_bytes(),
            );
            if content_hash != file.content_hash {
                self.schedule_vault_autosave(buffer_id);
            }
        }
    }

    pub(crate) fn flush_due_vault_autosaves(&mut self, should_force: bool) {
        if should_force {
            // A boundary flush (focus loss, tab/pane switch, tab close,
            // window destroy) must export *every* dirty vault buffer, not
            // only the ones already scheduled. Discover first so a buffer
            // edited within the last 100 ms tick — which has no pending
            // entry yet — is still captured before the boundary passes.
            // Without this, closing or switching right after a keystroke
            // leaves the on-disk file behind the (durable) database copy.
            self.discover_dirty_vault_buffers();
        }
        let now_ms = self.now_ms();
        self.sweep_stuck_autosaves_in_flight(now_ms);
        let due: Vec<BufferId> = self
            .vault
            .pending_autosaves
            .iter()
            .filter_map(|(buffer_id, pending)| {
                (should_force || pending.due_ms <= now_ms).then_some(*buffer_id)
            })
            .collect();
        for buffer_id in due {
            if self.vault.autosaves_in_flight.contains_key(&buffer_id) {
                continue;
            }
            let Some(snapshot) = self.editor.snapshot(buffer_id) else {
                self.vault.pending_autosaves.remove(&buffer_id);
                continue;
            };
            let Some(file) = snapshot.file.as_ref() else {
                self.vault.pending_autosaves.remove(&buffer_id);
                continue;
            };
            if !self.vault.owns_file(&file.path) {
                self.vault.pending_autosaves.remove(&buffer_id);
                continue;
            }
            let content = snapshot.rope_snapshot().rope().to_string();
            let content_hash = continuity_persist::fnv1a_64(content.as_bytes());
            if content_hash == file.content_hash {
                self.vault.pending_autosaves.remove(&buffer_id);
                continue;
            }
            let reply = self.file_open_tx.clone();
            let Some(file_io) = self.file_io.as_ref() else {
                self.file_banner = Some(FileBanner::new("Vault autosave is unavailable".into()));
                continue;
            };
            if file_io.save_buffer(
                buffer_id,
                file.path.clone(),
                content,
                Some(file.hash),
                Some(reply),
            ) {
                self.vault.pending_autosaves.remove(&buffer_id);
                self.vault.autosaves_in_flight.insert(buffer_id, now_ms);
            } else {
                self.file_banner = Some(FileBanner::new(
                    "Vault autosave worker is unavailable".into(),
                ));
            }
        }
    }

    pub(crate) fn complete_vault_autosave(&mut self, buffer_id: BufferId) -> bool {
        self.vault.resume_autosave(buffer_id);
        self.vault.autosaves_in_flight.remove(&buffer_id).is_some()
    }

    /// Watchdog: clear any autosave that has been `in_flight` far longer than a
    /// write can plausibly take. A normal save acknowledges in milliseconds; an
    /// entry older than [`STUCK_AUTOSAVE_MS`] means the completion event was
    /// lost, so drop it and let the next tick re-dispatch instead of wedging
    /// autosave for that buffer permanently.
    fn sweep_stuck_autosaves_in_flight(&mut self, now_ms: u64) {
        let stuck: Vec<BufferId> = self
            .vault
            .autosaves_in_flight
            .iter()
            .filter_map(|(buffer_id, since_ms)| {
                (now_ms.saturating_sub(*since_ms) > STUCK_AUTOSAVE_MS).then_some(*buffer_id)
            })
            .collect();
        for buffer_id in stuck {
            self.vault.autosaves_in_flight.remove(&buffer_id);
        }
    }

    pub(crate) fn fail_vault_autosave(&mut self, buffer_id: BufferId) {
        self.vault.autosaves_in_flight.remove(&buffer_id);
    }

    /// Whether a refused save (`SaveConflict`) still diverges from disk once
    /// any optimistic clean-state rollback has been applied — i.e. the disk
    /// fingerprint the worker just read still differs from what this buffer
    /// last synced. Returns `false` when the conflict already resolved itself
    /// (an in-flight autosave or racing manual save advanced the association
    /// to match disk first), so autosave is *not* suspended and vault export
    /// keeps running. Mirrors the no-op condition in
    /// [`crate::window_file_reconcile::classify_reconcile`].
    pub(crate) fn is_save_conflict_unresolved(
        &self,
        buffer_id: BufferId,
        disk: &continuity_buffer::FileAssociation,
    ) -> bool {
        match self
            .editor
            .snapshot(buffer_id)
            .and_then(|snapshot| snapshot.file)
        {
            Some(stored) => fingerprints_diverge(
                stored.hash,
                stored.content_hash,
                disk.hash,
                disk.content_hash,
            ),
            None => false,
        }
    }

    pub(crate) fn suspend_vault_autosave(&mut self, buffer_id: BufferId) {
        let is_owned = self
            .editor
            .snapshot(buffer_id)
            .and_then(|snapshot| snapshot.file)
            .is_some_and(|file| self.vault.owns_file(&file.path));
        if is_owned {
            self.vault.suspend_autosave(buffer_id);
        }
    }
}

/// Whether two file fingerprints differ in either the raw-byte hash or the
/// decoded-content hash. Extracted for direct unit testing of the
/// save-conflict resolution decision.
fn fingerprints_diverge(
    stored_hash: u64,
    stored_content_hash: u64,
    disk_hash: u64,
    disk_content_hash: u64,
) -> bool {
    disk_hash != stored_hash || disk_content_hash != stored_content_hash
}

#[cfg(test)]
mod tests {
    use super::fingerprints_diverge;

    #[test]
    fn resolved_conflict_does_not_diverge() {
        // Disk fingerprint equals the stored association (an in-flight save or
        // racing Ctrl+S already matched disk) → not a real conflict → autosave
        // must stay live.
        assert!(!fingerprints_diverge(10, 20, 10, 20));
    }

    #[test]
    fn real_conflict_diverges_on_either_hash() {
        // Raw-byte hash differs (external edit) → real conflict → suspend.
        assert!(fingerprints_diverge(10, 20, 11, 20));
        // Decoded-content hash differs (re-encode) → real conflict → suspend.
        assert!(fingerprints_diverge(10, 20, 10, 21));
    }
}
