//! File-I/O completion dispatch on one window's UI thread.

use crate::file_io::FileIoEvent;
use crate::window::Window;
use crate::window_file::FileBanner;

impl Window {
    pub(crate) fn handle_file_io_event(&mut self, event: FileIoEvent) {
        match event {
            FileIoEvent::FolderInspected {
                requested_root,
                root,
                config,
                workspace,
            } => self.handle_folder_inspected(requested_root, root, config, workspace),
            FileIoEvent::VaultEntriesChanged {
                root,
                refresh_relative,
                moved,
                deleted,
            } => self.handle_vault_entries_changed(root, refresh_relative, moved, deleted),
            FileIoEvent::VaultConfigChanged { root, config } => {
                self.handle_vault_config_changed(root, config)
            }
            FileIoEvent::Opened {
                target_pane,
                content,
                file,
                disposition,
            } => self.handle_opened_file(target_pane, content, file, disposition),
            FileIoEvent::DirectoryListed {
                root,
                relative,
                entries,
                truncated,
            } => self.handle_file_tree_directory_list(root, relative, entries, truncated),
            FileIoEvent::Saved { buffer_id, file } => self.handle_file_saved(buffer_id, file),
            FileIoEvent::SaveConflict {
                buffer_id,
                path: _,
                content,
                file,
            } => {
                self.fail_vault_autosave(buffer_id);
                if let Some(baseline) = self.pending_save_baseline.remove(&buffer_id) {
                    if let Some(stored) = self.editor.snapshot(buffer_id).and_then(|s| s.file) {
                        let _ = self.editor.set_file_association(
                            buffer_id,
                            Some(stored.with_content_hash(baseline)),
                        );
                    }
                }
                // Suspend autosave only for a conflict that is still real once
                // any optimistic clean-state rollback has been undone. A
                // conflict that already resolved itself — an in-flight autosave
                // or a racing Ctrl+S landed and advanced the association to
                // match disk before this refused write was processed — must
                // keep autosave live. Suspending unconditionally left the
                // buffer stuck "unsaved" forever with no banner (the reconcile
                // below is a no-op when disk already matches), which silently
                // stalled vault export mid-session.
                if self.is_save_conflict_unresolved(buffer_id, &file) {
                    self.suspend_vault_autosave(buffer_id);
                }
                self.reconcile_after_save_conflict(buffer_id, content, file);
            }
            FileIoEvent::Reloaded {
                buffer_id,
                content,
                file,
            } => self.apply_reloaded_file(buffer_id, content, file),
            FileIoEvent::ExternalChanged {
                buffer_id,
                path: _,
                content,
                file,
            }
            | FileIoEvent::Rechecked {
                buffer_id,
                content,
                file,
            } => {
                self.reconcile_file_buffer(buffer_id, content, file);
            }
            FileIoEvent::Deleted { buffer_id, path } => {
                if self
                    .tree
                    .tabs
                    .values()
                    .any(|tab| tab.buffer_id == buffer_id)
                {
                    self.file_banner = Some(FileBanner::new(format!(
                        "{} was deleted externally — buffer kept in memory. Save to recreate.",
                        path.display()
                    )));
                }
            }
            FileIoEvent::EncodingNotice { path, encoding } => {
                self.file_banner = Some(FileBanner::new(format!(
                    "{} appears to be {encoding} — opened with replacement characters. \
                     Re-save will write UTF-8 and discard the original encoding.",
                    path.display()
                )));
            }
            FileIoEvent::Failed {
                buffer_id,
                operation,
                path,
                reason,
            } => {
                if operation == "rename tree entry" {
                    self.file_tree.mark_rename_failed();
                    self.invalidate(self.hwnd);
                }
                self.handle_file_failed(buffer_id, operation, path, reason);
            }
        }
    }

    fn handle_file_saved(
        &mut self,
        buffer_id: continuity_buffer::BufferId,
        file: continuity_buffer::FileAssociation,
    ) {
        let was_vault_autosave = self.complete_vault_autosave(buffer_id);
        self.pending_save_baseline.remove(&buffer_id);
        let _ = self
            .editor
            .set_file_association(buffer_id, Some(file.clone()));
        self.mark_tab_file_associated(buffer_id, &file);
        self.decoration_cache.evict(buffer_id.as_uuid().as_u128());
        self.last_submitted_decoration_revision_per_buffer
            .borrow_mut()
            .remove(&buffer_id);
        if buffer_id == self.buffer_id {
            self.language_revision = None;
            self.last_submitted_decoration_revision = None;
            self.refresh_language();
            self.maybe_submit_decoration();
        }
        if let Some(register) = self.register_file_buffer.as_ref() {
            register(buffer_id, file.clone());
        }
        let now = self.now_ms();
        if !was_vault_autosave {
            self.file_banner = Some(FileBanner::transient(
                format!("Saved {}", file.path.display()),
                now,
            ));
            let label = file
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            crate::window_status_notice::push_save_confirm_notice(
                &mut self.status_notices,
                label,
                now,
            );
        }
        self.start_motion_timer();
        if let Some(file_io) = self.file_io.as_ref() {
            let _ = file_io.watch_file(buffer_id, file);
        }
    }

    fn handle_file_failed(
        &mut self,
        buffer_id: Option<continuity_buffer::BufferId>,
        operation: &'static str,
        path: Option<std::path::PathBuf>,
        reason: String,
    ) {
        if let Some(path) = path.as_ref() {
            self.note_vault_tab_restore_failure(path);
        }
        if let Some(buffer_id) = buffer_id {
            self.fail_vault_autosave(buffer_id);
        }
        if let Some(buffer_id) = buffer_id {
            if let Some(baseline) = self.pending_save_baseline.remove(&buffer_id) {
                if let Some(file) = self.editor.snapshot(buffer_id).and_then(|snap| snap.file) {
                    let _ = self
                        .editor
                        .set_file_association(buffer_id, Some(file.with_content_hash(baseline)));
                }
            }
        }
        let label = path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "file".into());
        self.file_banner = Some(FileBanner::new(format!(
            "{operation} failed for {label}: {reason}"
        )));
    }
}
