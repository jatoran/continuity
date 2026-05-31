//! File-command dispatch surface.
//!
//! Split from [`crate::view_context`] so view toggles, pane commands,
//! and file commands do not crowd one trait file. The production
//! implementor is `ui::Window`, which owns the UI thread state and
//! delegates disk work to its file-I/O worker.

use std::path::PathBuf;

use crate::Error;

/// File interaction command surface.
pub trait FileContext {
    /// Open the native file picker and import selected files.
    fn file_open_dialog(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_open_dialog"))
    }

    /// Import the supplied file paths.
    fn file_open_paths(&mut self, _paths: Vec<PathBuf>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_open_paths"))
    }

    /// Open a folder in the left file-tree pane. `None` opens a native
    /// folder picker.
    fn file_open_folder(&mut self, _path: Option<PathBuf>) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_open_folder"))
    }

    /// Toggle visibility of the left file-tree pane.
    fn toggle_file_tree(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("toggle_file_tree"))
    }

    /// Save the active buffer.
    fn file_save(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_save"))
    }

    /// Save the active buffer to a selected path.
    fn file_save_as(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_save_as"))
    }

    /// Reload the file named by the active external-change banner.
    fn file_reload_external(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_reload_external"))
    }

    /// Keep editor content and dismiss the active external-change banner.
    fn file_keep_mine(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_keep_mine"))
    }

    /// Show a diff for the active external-change banner.
    fn file_show_diff(&mut self) -> Result<(), Error> {
        Err(Error::UnsupportedContext("file_show_diff"))
    }
}
