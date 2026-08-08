//! Inline file-tree rename keyboard routing and worker dispatch.
//!
//! The UI thread owns the transient text field and focus. Filesystem mutation
//! is sent to the file-I/O worker so the HWND thread never blocks on disk I/O.

use std::path::PathBuf;

use continuity_input::KeyChord;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_A, VK_BACK, VK_C, VK_DELETE, VK_END, VK_ESCAPE, VK_F2, VK_HOME, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_TAB, VK_V, VK_X,
};

use crate::text_input::InputChord;
use crate::window_file::FileBanner;
use crate::window_helpers::invalidate_hwnd_with_reason;
use crate::Window;

impl Window {
    pub(crate) fn try_file_tree_rename_char(&mut self, character: char) -> bool {
        if !self.file_tree.has_keyboard_focus() {
            return false;
        }
        if let Some(rename) = self.file_tree.rename_mut() {
            if !rename.is_pending() {
                rename.input_mut().insert_char(character);
                invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_character");
            }
        }
        true
    }

    pub(crate) fn try_file_tree_rename_keydown(
        &mut self,
        virtual_key: u16,
        chord: &KeyChord,
    ) -> bool {
        if !self.file_tree.has_keyboard_focus() || chord.modifiers.alt || chord.modifiers.meta {
            return false;
        }
        if self.file_tree.rename().is_none() {
            if virtual_key == VK_F2.0 {
                self.begin_file_tree_rename();
            } else if virtual_key == VK_ESCAPE.0 {
                self.file_tree.set_keyboard_focus(false);
                invalidate_hwnd_with_reason(self.hwnd, "file_tree_keyboard_blur");
            }
            return true;
        }
        if self
            .file_tree
            .rename()
            .is_some_and(crate::file_tree::FileTreeRenameState::is_pending)
        {
            if virtual_key == VK_ESCAPE.0 {
                self.file_tree.cancel_rename();
                invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_cancel_pending");
            }
            return true;
        }

        let ctrl = chord.modifiers.ctrl;
        let shift = chord.modifiers.shift;
        if ctrl && !shift {
            match virtual_key {
                key if key == VK_A.0 => self.apply_rename_input_chord(InputChord::SelectAll),
                key if key == VK_C.0 => self.copy_file_tree_rename_selection(),
                key if key == VK_X.0 => self.cut_file_tree_rename_selection(),
                key if key == VK_V.0 => self.paste_file_tree_rename_selection(),
                _ => {}
            }
            return true;
        }
        if shift && !ctrl {
            let input_chord = match virtual_key {
                key if key == VK_LEFT.0 => Some(InputChord::ExtendLeft),
                key if key == VK_RIGHT.0 => Some(InputChord::ExtendRight),
                key if key == VK_HOME.0 => Some(InputChord::ExtendHome),
                key if key == VK_END.0 => Some(InputChord::ExtendEnd),
                _ => None,
            };
            if let Some(input_chord) = input_chord {
                self.apply_rename_input_chord(input_chord);
            }
            return true;
        }

        match virtual_key {
            key if key == VK_RETURN.0 => {
                let _ = self.commit_file_tree_rename();
            }
            key if key == VK_TAB.0 => {
                let _ = self.commit_file_tree_rename();
                self.file_tree.set_keyboard_focus(false);
            }
            key if key == VK_ESCAPE.0 => {
                self.file_tree.cancel_rename();
                invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_cancel");
            }
            key if key == VK_BACK.0 => self.mutate_rename_input(|input| input.delete_back()),
            key if key == VK_DELETE.0 => self.mutate_rename_input(|input| input.delete_forward()),
            key if key == VK_LEFT.0 => self.mutate_rename_input(|input| input.move_left()),
            key if key == VK_RIGHT.0 => self.mutate_rename_input(|input| input.move_right()),
            key if key == VK_HOME.0 => self.mutate_rename_input(|input| {
                input.move_home();
                true
            }),
            key if key == VK_END.0 => self.mutate_rename_input(|input| {
                input.move_end();
                true
            }),
            _ => {}
        }
        true
    }

    pub(crate) fn begin_file_tree_rename(&mut self) {
        if self.file_tree.begin_rename_selected() {
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_begin");
        }
    }

    pub(crate) fn commit_file_tree_rename(&mut self) -> bool {
        let Some(rename) = self.file_tree.rename() else {
            return false;
        };
        let source = rename.relative().to_path_buf();
        let new_name = rename.input().text.clone();
        if let Err(error) = crate::file_io_vault_entries::validate_entry_name(&new_name) {
            self.file_banner = Some(FileBanner::new(format!("Rename failed: {error}")));
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_invalid");
            return true;
        }
        let old_name = source.file_name().and_then(|name| name.to_str());
        if old_name == Some(new_name.as_str()) {
            self.file_tree.cancel_rename();
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_unchanged");
            return true;
        }
        let (Some(root), Some(file_io)) = (
            self.file_tree.root().map(PathBuf::from),
            self.file_io.as_ref(),
        ) else {
            self.file_banner = Some(FileBanner::new("File I/O is not available".into()));
            return true;
        };
        if file_io.rename_tree_entry(root, source, new_name, self.file_open_tx.clone()) {
            self.file_tree.mark_rename_pending();
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_requested");
        } else {
            self.file_banner = Some(FileBanner::new("Rename request failed".into()));
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_request_failed");
        }
        true
    }

    fn apply_rename_input_chord(&mut self, input_chord: InputChord) {
        self.mutate_rename_input(|input| input.apply_input_chord(input_chord));
    }

    fn mutate_rename_input(
        &mut self,
        mutate: impl FnOnce(&mut crate::text_input::TextInput) -> bool,
    ) {
        if self
            .file_tree
            .rename_mut()
            .is_some_and(|rename| mutate(rename.input_mut()))
        {
            invalidate_hwnd_with_reason(self.hwnd, "file_tree_rename_input");
        }
    }

    fn copy_file_tree_rename_selection(&mut self) {
        let selected = self.file_tree.rename().and_then(|rename| {
            rename
                .input()
                .selection_text()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        });
        if let Some(selected) = selected {
            let _ = self.request_host_clipboard_write(&selected);
        }
    }

    fn cut_file_tree_rename_selection(&mut self) {
        let selected = self.file_tree.rename().and_then(|rename| {
            rename
                .input()
                .selection_text()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        });
        if let Some(selected) = selected {
            let _ = self.request_host_clipboard_write(&selected);
            self.mutate_rename_input(|input| input.replace_selection(""));
        }
    }

    fn paste_file_tree_rename_selection(&mut self) {
        let Ok(Some(raw)) = self.request_host_clipboard_read() else {
            return;
        };
        let single_line: String = raw
            .chars()
            .filter(|character| *character != '\r' && *character != '\n')
            .collect();
        if !single_line.is_empty() {
            self.mutate_rename_input(|input| {
                input.insert_str(&single_line);
                true
            });
        }
    }
}
