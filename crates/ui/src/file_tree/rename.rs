//! UI-thread inline rename state for the file tree.

use std::path::{Path, PathBuf};

use continuity_render::FileTreeInlineEditDraw;

use crate::text_input::TextInput;

use super::{FileTreeNodeKind, FileTreeState};

#[derive(Clone, Debug)]
pub(crate) struct FileTreeRenameState {
    relative: PathBuf,
    input: TextInput,
    pending: bool,
}

impl FileTreeRenameState {
    fn new(relative: PathBuf, is_file: bool) -> Option<Self> {
        let name = relative.file_name()?.to_string_lossy().into_owned();
        let mut input = TextInput::new();
        input.set_text(name.clone());
        let selection_end = if is_file {
            Path::new(&name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map_or(name.len(), str::len)
        } else {
            name.len()
        };
        input.selection_anchor = Some(0);
        input.caret = selection_end;
        Some(Self {
            relative,
            input,
            pending: false,
        })
    }

    pub(crate) fn relative(&self) -> &Path {
        &self.relative
    }

    pub(crate) fn input(&self) -> &TextInput {
        &self.input
    }

    pub(crate) fn input_mut(&mut self) -> &mut TextInput {
        &mut self.input
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }

    pub(crate) fn build_draw(&self) -> FileTreeInlineEditDraw {
        FileTreeInlineEditDraw {
            text: self.input.text.clone(),
            caret_byte: self.input.caret,
            selection_range: self.input.selection_range(),
        }
    }
}

impl FileTreeState {
    pub(crate) fn has_keyboard_focus(&self) -> bool {
        self.keyboard_focused
    }

    pub(crate) fn set_keyboard_focus(&mut self, focused: bool) {
        self.keyboard_focused = focused;
    }

    pub(crate) fn begin_rename_selected(&mut self) -> bool {
        let Some(relative) = self.selected.clone() else {
            return false;
        };
        let Some(node) = self.nodes.get(&relative) else {
            return false;
        };
        self.rename = FileTreeRenameState::new(relative, node.kind == FileTreeNodeKind::File);
        self.keyboard_focused = self.rename.is_some();
        self.rename.is_some()
    }

    pub(crate) fn rename(&self) -> Option<&FileTreeRenameState> {
        self.rename.as_ref()
    }

    pub(crate) fn rename_mut(&mut self) -> Option<&mut FileTreeRenameState> {
        self.rename.as_mut()
    }

    pub(crate) fn cancel_rename(&mut self) -> bool {
        self.rename.take().is_some()
    }

    pub(crate) fn mark_rename_pending(&mut self) {
        if let Some(rename) = self.rename.as_mut() {
            rename.pending = true;
        }
    }

    pub(crate) fn mark_rename_failed(&mut self) {
        if let Some(rename) = self.rename.as_mut() {
            rename.pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DirectoryEntry, DirectoryEntryKind};

    #[test]
    fn file_rename_selects_stem_but_keeps_extension_visible() {
        let root = PathBuf::from(r"C:\Vault");
        let mut tree = FileTreeState::default();
        tree.open_root(root.clone(), None);
        tree.apply_directory_list(
            &root,
            PathBuf::new(),
            vec![DirectoryEntry {
                relative: PathBuf::from("note.md"),
                name: "note".into(),
                kind: DirectoryEntryKind::File,
                size_bytes: Some(0),
            }],
            false,
        );
        tree.select(PathBuf::from("note.md"));
        assert!(tree.begin_rename_selected());
        let input = tree.rename().expect("rename state").input();
        assert_eq!(input.text, "note.md");
        assert_eq!(input.selection_range(), Some((0, 4)));
    }

    #[test]
    fn directory_rename_selects_the_complete_name() {
        let root = PathBuf::from(r"C:\Vault");
        let mut tree = FileTreeState::default();
        tree.open_root(root.clone(), None);
        tree.apply_directory_list(
            &root,
            PathBuf::new(),
            vec![DirectoryEntry {
                relative: PathBuf::from("Daily Notes"),
                name: "Daily Notes".into(),
                kind: DirectoryEntryKind::Directory,
                size_bytes: None,
            }],
            false,
        );
        tree.select(PathBuf::from("Daily Notes"));
        assert!(tree.begin_rename_selected());
        let input = tree.rename().expect("rename state").input();
        assert_eq!(input.text, "Daily Notes");
        assert_eq!(input.selection_range(), Some((0, 11)));
    }
}
