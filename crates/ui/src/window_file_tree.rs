//! File-tree command, mouse, and worker-event integration.
//!
//! The window owns only UI state here. Directory reads stay on
//! `file_io`; file opens go through the existing file-open path so
//! buffers, tabs, watchers, and encoding banners remain consistent.

use std::path::PathBuf;

use continuity_render::{EditorColors, FileTreeDragDraw, FileTreeDraw};

use crate::file_tree::FILE_TREE_MAX_OPEN_BYTES;
use crate::window::Window;
use crate::window_config::FileOpenDisposition;
use crate::window_file::FileBanner;
use crate::window_file_dialogs::open_folder_dialog;
use crate::window_helpers::{invalidate_hwnd, invalidate_hwnd_with_reason};

impl Window {
    pub(crate) fn open_vault_settings(&mut self) -> Result<(), continuity_command::Error> {
        let root = self
            .vault
            .root()
            .ok_or(continuity_command::Error::UnsupportedContext(
                "open_vault_settings",
            ))?;
        let config_path = root
            .join(continuity_config::VAULT_CONFIG_DIRECTORY)
            .join(continuity_config::VAULT_CONFIG_FILE);
        self.file_open_files_or_folder_paths(vec![config_path], FileOpenDisposition::NewTab)
    }

    pub(crate) fn create_vault_entry_from_tree(
        &mut self,
        parent: PathBuf,
        kind: crate::file_io::VaultEntryKind,
    ) {
        let (Some(root), Some(file_io)) =
            (self.vault.root().map(PathBuf::from), self.file_io.as_ref())
        else {
            return;
        };
        if !file_io.create_vault_entry(root, parent, kind, self.file_open_tx.clone()) {
            self.file_banner = Some(FileBanner::new("Vault create request failed".into()));
        }
    }

    pub(crate) fn delete_vault_entry_from_tree(&mut self, relative: PathBuf) {
        let now_ms = self.now_ms();
        if !self.vault.confirm_delete(&relative, now_ms) {
            self.file_banner = Some(FileBanner::transient_for(
                format!(
                    "Choose Move to Recycle Bin again within 3 seconds to delete {}",
                    relative.display()
                ),
                now_ms,
                3_000,
            ));
            return;
        }
        let (Some(root), Some(file_io)) =
            (self.vault.root().map(PathBuf::from), self.file_io.as_ref())
        else {
            return;
        };
        if !file_io.delete_vault_entry(root, relative, self.file_open_tx.clone()) {
            self.file_banner = Some(FileBanner::new("Vault delete request failed".into()));
        }
    }

    pub(crate) fn file_open_folder_impl(
        &mut self,
        path: Option<PathBuf>,
    ) -> Result<(), continuity_command::Error> {
        let Some(path) = path.or_else(|| open_folder_dialog(self.hwnd)) else {
            return Ok(());
        };
        self.open_folder_root(path)
    }

    pub(crate) fn toggle_file_tree_impl(&mut self) -> Result<(), crate::Error> {
        let visible = !self.file_tree.is_visible();
        self.file_tree.set_visible(visible);
        if visible {
            for relative in self.file_tree.expanded_directories_needing_load() {
                self.request_file_tree_directory(relative);
            }
        }
        self.persist_vault_workspace_state();
        self.refresh_after_file_tree_width_change("toggle_file_tree");
        self.request_state_save();
        invalidate_hwnd_with_reason(self.hwnd, "view_toggle_file_tree");
        Ok(())
    }

    pub(crate) fn open_folder_root(
        &mut self,
        path: PathBuf,
    ) -> Result<(), continuity_command::Error> {
        let file_io =
            self.file_io
                .as_ref()
                .ok_or(continuity_command::Error::UnsupportedContext(
                    "file_open_folder",
                ))?;
        if file_io.inspect_folder(path, self.file_open_tx.clone()) {
            Ok(())
        } else {
            Err(continuity_command::Error::UnsupportedContext(
                "file_open_folder",
            ))
        }
    }

    pub(crate) fn handle_folder_inspected(
        &mut self,
        requested_root: PathBuf,
        root: PathBuf,
        config: Option<continuity_config::VaultConfig>,
        workspace: Option<continuity_config::VaultWorkspaceState>,
    ) {
        let tree_root = if config.is_some() {
            root.clone()
        } else {
            requested_root.clone()
        };
        self.apply_vault_appearance(config.as_ref());
        self.vault
            .apply_inspection(requested_root, root.clone(), config.clone());
        if config.is_some() {
            self.vault.notify_activated(root.clone());
        }
        let first_relative = self.file_tree.open_root(tree_root, workspace.as_ref());
        self.request_file_tree_directory(first_relative);
        if config.is_some() {
            if let Some(workspace) = workspace.as_ref() {
                self.begin_vault_tab_restore(workspace);
            }
        }
        if config.is_none() {
            self.file_banner = Some(FileBanner::vault_offer(
                self.file_tree.root().map(PathBuf::from).unwrap_or_default(),
            ));
        } else if self
            .file_banner
            .as_ref()
            .is_some_and(|banner| banner.vault_initialization.is_some())
        {
            self.file_banner = None;
        }
        self.refresh_after_file_tree_width_change("open_folder");
        self.request_state_save();
        invalidate_hwnd_with_reason(self.hwnd, "file_tree_open_folder");
    }

    pub(crate) fn initialize_current_folder_as_vault(
        &mut self,
    ) -> Result<(), continuity_command::Error> {
        let root = self
            .file_banner
            .as_ref()
            .and_then(|banner| banner.vault_initialization.clone())
            .or_else(|| self.vault.browse_root().map(PathBuf::from))
            .ok_or(continuity_command::Error::UnsupportedContext(
                "initialize_vault",
            ))?;
        let file_io =
            self.file_io
                .as_ref()
                .ok_or(continuity_command::Error::UnsupportedContext(
                    "initialize_vault",
                ))?;
        if file_io.initialize_vault(root, self.file_open_tx.clone()) {
            self.file_banner = Some(FileBanner::new("Initializing vault…".into()));
            Ok(())
        } else {
            Err(continuity_command::Error::UnsupportedContext(
                "initialize_vault",
            ))
        }
    }

    pub(crate) fn file_open_files_or_folder_paths(
        &mut self,
        paths: Vec<PathBuf>,
        disposition: FileOpenDisposition,
    ) -> Result<(), continuity_command::Error> {
        let mut files = Vec::new();
        let mut first_folder = None;
        for path in paths {
            if path.is_dir() {
                if first_folder.is_none() {
                    first_folder = Some(path);
                }
            } else {
                files.push(path);
            }
        }
        if let Some(folder) = first_folder {
            self.open_folder_root(folder)?;
        }
        if files.is_empty() {
            return Ok(());
        }
        let file_io = self
            .file_io
            .as_ref()
            .ok_or(continuity_command::Error::UnsupportedContext("file_open"))?;
        if file_io.open_files_with_reply(
            files,
            Some(self.tree.focused),
            self.file_open_tx.clone(),
            self.hwnd.0 as usize,
            disposition,
        ) {
            Ok(())
        } else {
            Err(continuity_command::Error::UnsupportedContext("file_open"))
        }
    }

    /// Move the file-tree highlight onto the focused tab's file so the tree
    /// always reflects the buffer in view. Clears the highlight when the
    /// focused tab has no file backing or its file lives outside the tree
    /// root. Called each time the tree draw payload is built.
    fn sync_file_tree_selection_to_focused_tab(&mut self) {
        let Some(root) = self.file_tree.root().map(std::path::Path::to_path_buf) else {
            return;
        };
        let relative = self
            .tree
            .active_buffer_opt()
            .and_then(|buffer_id| self.editor.snapshot(buffer_id))
            .and_then(|snapshot| snapshot.file)
            .and_then(|file| {
                file.path
                    .strip_prefix(&root)
                    .ok()
                    .map(std::path::Path::to_path_buf)
            });
        self.file_tree.set_selected(relative);
    }

    pub(crate) fn build_file_tree_draw_payload(
        &mut self,
        colors: EditorColors,
    ) -> Option<FileTreeDraw> {
        self.sync_file_tree_selection_to_focused_tab();
        let status_height = if self.view_options.show_status_bar {
            continuity_render::STATUS_BAR_HEIGHT_DIP
        } else {
            0.0
        };
        let tree_height = (self.client_height_dip() - status_height).max(0.0);
        let mut draw = self
            .file_tree
            .build_draw(tree_height, colors, self.vault.config())?;
        if let Some(drag) = self
            .mouse_state
            .file_tree_entry_drag
            .as_ref()
            .filter(|drag| drag.is_dragging)
        {
            let drop_target_top_dip = self
                .file_tree
                .drop_target_top_at(drag.current_x as f32, drag.current_y as f32);
            draw.drag = Some(FileTreeDragDraw {
                label: drag
                    .relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("item")
                    .to_string(),
                cursor: (drag.current_x as f32, drag.current_y as f32),
                drop_target_top_dip,
            });
        }
        Some(draw)
    }

    pub(crate) fn handle_file_tree_directory_list(
        &mut self,
        root: PathBuf,
        relative: PathBuf,
        entries: Vec<crate::DirectoryEntry>,
        truncated: bool,
    ) {
        if let Some(directories_to_load) = self
            .file_tree
            .apply_directory_list(&root, relative, entries, truncated)
        {
            for directory in directories_to_load {
                self.request_file_tree_directory(directory);
            }
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_directory_listed");
        }
    }

    pub(crate) fn handle_vault_entries_changed(
        &mut self,
        root: PathBuf,
        refresh_relative: PathBuf,
        moved: Option<(PathBuf, PathBuf)>,
        deleted: Option<PathBuf>,
    ) {
        if self.file_tree.root() != Some(root.as_path()) {
            return;
        }
        let is_vault = self.vault.root() == Some(root.as_path());
        if let Some((source, destination)) = moved.as_ref() {
            let state_changed = self
                .file_tree
                .reassociate_expanded_path(source, destination);
            self.reassociate_moved_vault_buffers(&root, source, destination);
            let destination_parent = destination
                .parent()
                .unwrap_or(std::path::Path::new(""))
                .to_path_buf();
            if destination_parent != refresh_relative {
                self.request_file_tree_directory(destination_parent);
            }
            if is_vault && state_changed {
                self.persist_vault_workspace_state();
            }
        }
        if let Some(deleted) = deleted.as_ref() {
            let state_changed = self.file_tree.remove_expanded_path(deleted);
            self.detach_deleted_vault_buffers(&root, deleted);
            if is_vault && state_changed {
                self.persist_vault_workspace_state();
            }
        }
        self.request_file_tree_directory(refresh_relative);
    }

    pub(crate) fn handle_vault_config_changed(
        &mut self,
        root: PathBuf,
        config: continuity_config::VaultConfig,
    ) {
        if !self.vault.update_config(&root, config.clone()) {
            return;
        }
        self.apply_vault_appearance(Some(&config));
        self.request_file_tree_directory(PathBuf::new());
        invalidate_hwnd_with_reason(self.hwnd, "vault_config_changed");
    }

    fn reassociate_moved_vault_buffers(
        &mut self,
        root: &std::path::Path,
        source: &std::path::Path,
        destination: &std::path::Path,
    ) {
        let source_absolute = root.join(source);
        let destination_absolute = root.join(destination);
        let buffer_ids: Vec<_> = self.tree.tabs.values().map(|tab| tab.buffer_id).collect();
        for buffer_id in buffer_ids {
            let Some(mut file) = self
                .editor
                .snapshot(buffer_id)
                .and_then(|snapshot| snapshot.file)
            else {
                continue;
            };
            let Ok(suffix) = file.path.strip_prefix(&source_absolute) else {
                continue;
            };
            file.path = destination_absolute.join(suffix);
            let _ = self
                .editor
                .set_file_association(buffer_id, Some(file.clone()));
            if let Some(register) = self.register_file_buffer.as_ref() {
                register(buffer_id, file);
            }
        }
    }

    fn detach_deleted_vault_buffers(&mut self, root: &std::path::Path, deleted: &std::path::Path) {
        let deleted_absolute = root.join(deleted);
        let buffer_ids: Vec<_> = self.tree.tabs.values().map(|tab| tab.buffer_id).collect();
        for buffer_id in buffer_ids {
            let should_detach = self
                .editor
                .snapshot(buffer_id)
                .and_then(|snapshot| snapshot.file)
                .is_some_and(|file| file.path.starts_with(&deleted_absolute));
            if should_detach {
                self.vault.pending_autosaves.remove(&buffer_id);
                self.vault.autosaves_in_flight.remove(&buffer_id);
                self.vault.suspended_autosaves.remove(&buffer_id);
                let _ = self.editor.set_file_association(buffer_id, None);
            }
        }
    }

    pub(crate) fn request_file_tree_directory(&mut self, relative: PathBuf) {
        let Some(root) = self.file_tree.root().map(|root| root.to_path_buf()) else {
            return;
        };
        self.file_tree.mark_pending(relative.clone());
        let Some(file_io) = self.file_io.as_ref() else {
            self.file_tree.clear_pending(&relative);
            self.file_banner = Some(FileBanner::new("File I/O is not available".into()));
            return;
        };
        if !file_io.list_directory(
            root,
            relative.clone(),
            self.vault.config().cloned(),
            self.file_open_tx.clone(),
        ) {
            self.file_tree.clear_pending(&relative);
            self.file_banner = Some(FileBanner::new("File I/O worker is not available".into()));
        }
    }

    pub(crate) fn open_file_tree_file(
        &mut self,
        relative: PathBuf,
        size_bytes: Option<u64>,
        disposition: FileOpenDisposition,
    ) {
        if size_bytes.is_some_and(|size| size > FILE_TREE_MAX_OPEN_BYTES) {
            self.file_banner = Some(FileBanner::new(format!(
                "File is larger than {} MiB; open it explicitly to avoid a slow import.",
                FILE_TREE_MAX_OPEN_BYTES / (1024 * 1024)
            )));
            invalidate_hwnd(self.hwnd);
            return;
        }
        let Some(path) = self.file_tree.absolute_path(&relative) else {
            return;
        };
        if let Err(err) = self.file_open_paths_with_disposition(vec![path], disposition) {
            self.file_banner = Some(FileBanner::new(err.to_string()));
            invalidate_hwnd(self.hwnd);
        }
    }

    fn refresh_after_file_tree_width_change(&mut self, reason: &'static str) {
        self.clear_right_edge_layout_caches();
        self.refresh_focused_viewport();
        let _ = self.try_dispatch_projection_worker_early(reason, "layout_change");
    }
}
