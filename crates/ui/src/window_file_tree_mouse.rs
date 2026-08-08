//! File-tree pointer handling: click-to-open, middle-click new tab,
//! entry drag/drop, and wheel scrolling.
//!
//! These handlers own only UI-thread state. File opens route through the
//! shared file-open path (`open_file_tree_file`) so buffers, tabs,
//! watchers, and encoding banners stay consistent; directory listing and
//! vault moves go through the file-I/O worker.

use std::path::PathBuf;

use continuity_render::FileTreeEntryKind;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};

use crate::window::Window;
use crate::window_config::FileOpenDisposition;
use crate::window_file::FileBanner;
use crate::window_helpers::invalidate_hwnd_with_reason;

impl Window {
    pub(crate) fn try_file_tree_left_down(&mut self, x: i32, y: i32, key_state: u32) -> bool {
        let Some(row) = self.file_tree.row_at(x as f32, y as f32) else {
            return false;
        };
        self.file_tree.set_keyboard_focus(true);
        if self
            .file_tree
            .rename()
            .is_some_and(|rename| rename.relative() == row.relative)
        {
            return true;
        }
        let _ = self.commit_file_tree_rename();
        match row.kind {
            FileTreeEntryKind::Directory => {
                self.file_tree.select(row.relative.clone());
                self.begin_file_tree_entry_drag(&row, x, y, FileOpenDisposition::Preview);
                if let Some(relative) = self.file_tree.toggle_directory(&row.relative) {
                    self.request_file_tree_directory(relative);
                }
                self.persist_vault_workspace_state();
                invalidate_hwnd_with_reason(self.hwnd, "file_tree_click_directory");
                true
            }
            FileTreeEntryKind::File => {
                self.file_tree.select(row.relative.clone());
                let disposition = compute_file_tree_open_disposition(key_state);
                self.begin_file_tree_entry_drag(&row, x, y, disposition);
                true
            }
            FileTreeEntryKind::Notice => true,
        }
    }

    /// `WM_MBUTTONDOWN` over the file tree: open the hovered file in a new
    /// tab, matching the Ctrl+click disposition. Directory and notice rows
    /// are ignored so the click falls through to the tab-strip handler.
    pub(crate) fn try_file_tree_middle_down(&mut self, x: i32, y: i32) -> bool {
        let Some(row) = self.file_tree.row_at(x as f32, y as f32) else {
            return false;
        };
        if row.kind != FileTreeEntryKind::File {
            return false;
        }
        let _ = self.commit_file_tree_rename();
        self.file_tree.select(row.relative.clone());
        self.open_file_tree_file(row.relative, row.size_bytes, FileOpenDisposition::NewTab);
        true
    }

    fn begin_file_tree_entry_drag(
        &mut self,
        row: &crate::file_tree::FileTreeHitRow,
        x: i32,
        y: i32,
        disposition: FileOpenDisposition,
    ) {
        self.mouse_state.file_tree_entry_drag = Some(crate::mouse::FileTreeEntryDrag {
            relative: row.relative.clone(),
            kind: row.kind,
            size_bytes: row.size_bytes,
            disposition,
            start_x: x,
            start_y: y,
            current_x: x,
            current_y: y,
            is_dragging: false,
        });
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = SetCapture(self.hwnd);
            }
        }
    }

    pub(crate) fn drag_file_tree_entry(&mut self, x: i32, y: i32) -> bool {
        let Some(drag) = self.mouse_state.file_tree_entry_drag.as_mut() else {
            return false;
        };
        if (x - drag.start_x).abs() >= 4 || (y - drag.start_y).abs() >= 4 {
            drag.is_dragging = true;
        }
        drag.current_x = x;
        drag.current_y = y;
        drag.is_dragging
    }

    pub(crate) fn finish_file_tree_entry_drag(&mut self, x: i32, y: i32) -> bool {
        let Some(drag) = self.mouse_state.file_tree_entry_drag.take() else {
            return false;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        if !drag.is_dragging {
            if drag.kind == FileTreeEntryKind::File {
                self.open_file_tree_file(drag.relative, drag.size_bytes, drag.disposition);
            }
            return true;
        }
        let Some(drop_row) = self.file_tree.row_at(x as f32, y as f32) else {
            return true;
        };
        let destination_directory = match drop_row.kind {
            FileTreeEntryKind::Directory => drop_row.relative,
            FileTreeEntryKind::File => drop_row
                .relative
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default(),
            FileTreeEntryKind::Notice => return true,
        };
        let (Some(root), Some(file_io)) =
            (self.vault.root().map(PathBuf::from), self.file_io.as_ref())
        else {
            return true;
        };
        if !file_io.move_vault_entry(
            root,
            drag.relative,
            destination_directory,
            self.file_open_tx.clone(),
        ) {
            self.file_banner = Some(FileBanner::new("Vault move request failed".into()));
        }
        true
    }

    pub(crate) fn try_file_tree_mouse_wheel(&mut self, x: i32, y: i32, notches: f32) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        if x as f32 >= self.file_tree.visible_width_dip() {
            return false;
        }
        if self.file_tree.scroll_by_notches(
            notches,
            (self.client_height_dip()
                - if self.view_options.show_status_bar {
                    continuity_render::STATUS_BAR_HEIGHT_DIP
                } else {
                    0.0
                })
            .max(0.0),
        ) {
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_scroll");
        }
        true
    }
}

fn compute_file_tree_open_disposition(key_state: u32) -> FileOpenDisposition {
    if key_state & 0x0004 != 0 {
        FileOpenDisposition::NewWindow
    } else if key_state & 0x0008 != 0 {
        FileOpenDisposition::NewTab
    } else {
        FileOpenDisposition::Preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tree_modifiers_select_open_disposition() {
        assert_eq!(
            compute_file_tree_open_disposition(0),
            FileOpenDisposition::Preview
        );
        assert_eq!(
            compute_file_tree_open_disposition(0x0008),
            FileOpenDisposition::NewTab
        );
        assert_eq!(
            compute_file_tree_open_disposition(0x0004),
            FileOpenDisposition::NewWindow
        );
        assert_eq!(
            compute_file_tree_open_disposition(0x000c),
            FileOpenDisposition::NewWindow
        );
    }
}
