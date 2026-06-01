//! File-tree command, mouse, and worker-event integration.
//!
//! The window owns only UI state here. Directory reads stay on
//! `file_io`; file opens go through the existing file-open path so
//! buffers, tabs, watchers, and encoding banners remain consistent.

use std::path::PathBuf;

use continuity_render::{EditorColors, FileTreeDraw, FileTreeEntryKind};

use crate::file_tree::FILE_TREE_MAX_OPEN_BYTES;
use crate::window::Window;
use crate::window_file::FileBanner;
use crate::window_file_dialogs::open_folder_dialog;
use crate::window_helpers::{invalidate_hwnd, invalidate_hwnd_with_reason};

impl Window {
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
        self.refresh_after_file_tree_width_change("toggle_file_tree");
        invalidate_hwnd_with_reason(self.hwnd, "view_toggle_file_tree");
        Ok(())
    }

    pub(crate) fn open_folder_root(
        &mut self,
        path: PathBuf,
    ) -> Result<(), continuity_command::Error> {
        let root = path.canonicalize().unwrap_or(path);
        if !root.is_dir() {
            return Err(continuity_command::Error::Other(format!(
                "{} is not a folder",
                root.display()
            )));
        }
        let first_relative = self.file_tree.open_root(root);
        self.request_file_tree_directory(first_relative);
        self.refresh_after_file_tree_width_change("open_folder");
        invalidate_hwnd_with_reason(self.hwnd, "file_tree_open_folder");
        Ok(())
    }

    pub(crate) fn file_open_files_or_folder_paths(
        &mut self,
        paths: Vec<PathBuf>,
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
        if file_io.open_files_with_reply(files, Some(self.tree.focused), self.file_open_tx.clone())
        {
            Ok(())
        } else {
            Err(continuity_command::Error::UnsupportedContext("file_open"))
        }
    }

    pub(crate) fn build_file_tree_draw_payload(
        &mut self,
        colors: EditorColors,
    ) -> Option<FileTreeDraw> {
        self.file_tree.build_draw(self.client_height_dip(), colors)
    }

    pub(crate) fn handle_file_tree_directory_list(
        &mut self,
        root: PathBuf,
        relative: PathBuf,
        entries: Vec<crate::DirectoryEntry>,
        truncated: bool,
    ) {
        if self
            .file_tree
            .apply_directory_list(&root, relative, entries, truncated)
        {
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_directory_listed");
        }
    }

    pub(crate) fn try_file_tree_left_down(&mut self, x: i32, y: i32) -> bool {
        let Some(row) = self.file_tree.row_at(x as f32, y as f32) else {
            return false;
        };
        match row.kind {
            FileTreeEntryKind::Directory => {
                self.file_tree.select(row.relative.clone());
                if let Some(relative) = self.file_tree.toggle_directory(&row.relative) {
                    self.request_file_tree_directory(relative);
                }
                invalidate_hwnd_with_reason(self.hwnd, "file_tree_click_directory");
                true
            }
            FileTreeEntryKind::File => {
                self.file_tree.select(row.relative.clone());
                self.open_file_tree_file(row.relative, row.size_bytes);
                true
            }
            FileTreeEntryKind::Notice => true,
        }
    }

    pub(crate) fn try_file_tree_mouse_wheel(&mut self, x: i32, y: i32, notches: f32) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        if x as f32 >= self.file_tree.visible_width_dip() {
            return false;
        }
        if self
            .file_tree
            .scroll_by_notches(notches, self.client_height_dip())
        {
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_scroll");
        }
        true
    }

    fn request_file_tree_directory(&mut self, relative: PathBuf) {
        let Some(root) = self.file_tree.root().map(|root| root.to_path_buf()) else {
            return;
        };
        self.file_tree.mark_pending(relative.clone());
        let Some(file_io) = self.file_io.as_ref() else {
            self.file_tree.clear_pending(&relative);
            self.file_banner = Some(FileBanner::new("File I/O is not available".into()));
            return;
        };
        if !file_io.list_directory(root, relative.clone()) {
            self.file_tree.clear_pending(&relative);
            self.file_banner = Some(FileBanner::new("File I/O worker is not available".into()));
        }
    }

    fn open_file_tree_file(&mut self, relative: PathBuf, size_bytes: Option<u64>) {
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
        if let Err(err) = self.file_open_paths_impl(vec![path]) {
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
