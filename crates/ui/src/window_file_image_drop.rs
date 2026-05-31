//! Phase F5 — drag-drop image-import path.
//!
//! Split out of [`crate::window_file`] so that file stays under the
//! 600-line cap. The drop dispatch (`Window::on_drop_files`) still
//! lives in `window_file.rs`; this sibling owns the image branch:
//! the per-extension predicate that decides whether a dropped path
//! is an image, and the [`Window::import_dropped_images`] mutator that
//! hash-dedupes each path into the shared image store and inserts the
//! markdown reference at the caret of the drop-target pane.
//!
//! Thread ownership: every mutator here runs on the window's UI
//! thread (drop messages dispatch from `WM_DROPFILES`).

use std::path::{Path, PathBuf};

use crate::image_store::{import_path, is_supported_image_extension};
use crate::pane_tree::PaneId;
use crate::window::Window;
use crate::window_file::FileBanner;

/// Predicate matching the conservative image-extension set
/// [`crate::image_store::SUPPORTED_IMAGE_EXTENSIONS`] against a
/// dropped path. Paths without an extension or with an extension
/// outside the set keep the legacy tab-open route.
#[must_use]
pub(crate) fn is_dropped_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_supported_image_extension)
}

impl Window {
    /// Import each path through the shared image store and insert a
    /// `![](images/<hash>.<ext>)` markdown reference at the caret of
    /// the drop-target pane (focusing the pane first so the
    /// insertion lands on the buffer the user dropped on). One
    /// markdown reference is appended per imported image; each
    /// insert is its own undo group (the selection-edit machinery
    /// enforces this).
    pub(crate) fn import_dropped_images(&mut self, paths: Vec<PathBuf>, target: Option<PaneId>) {
        let Some(images_dir) = self.image_store_dir.clone() else {
            self.file_banner = Some(FileBanner::new(
                "Image store unavailable - check `[markdown].images_dir`".into(),
            ));
            return;
        };
        if let Some(pane) = target {
            if self.tree.focused != pane {
                self.switch_focus(pane);
            }
        }
        for path in paths {
            match import_path(&path, &images_dir) {
                Ok(imported) => {
                    let markdown = format!("![]({})", imported.markdown_reference);
                    let _ = self.editor.apply_selection_edit(
                        self.buffer_id,
                        continuity_core::SelectionEdit::InsertText(markdown),
                    );
                }
                Err(err) => {
                    self.file_banner = Some(FileBanner::new(format!("Image import failed: {err}")));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn predicate_accepts_known_extensions_case_insensitive() {
        assert!(is_dropped_image_path(Path::new("foo.png")));
        assert!(is_dropped_image_path(Path::new("FOO.JPG")));
        assert!(is_dropped_image_path(Path::new("a.jpeg")));
        assert!(is_dropped_image_path(Path::new("a.gif")));
        assert!(is_dropped_image_path(Path::new("a.webp")));
        assert!(is_dropped_image_path(Path::new("a.bmp")));
    }

    #[test]
    fn predicate_rejects_non_image_and_unsupported() {
        assert!(!is_dropped_image_path(Path::new("foo.md")));
        assert!(!is_dropped_image_path(Path::new("foo.txt")));
        assert!(!is_dropped_image_path(Path::new("noext")));
        // SVG is intentionally NOT in the F5 raster set — vectors
        // need a different render path; route as plain file open.
        assert!(!is_dropped_image_path(Path::new("foo.svg")));
    }
}
