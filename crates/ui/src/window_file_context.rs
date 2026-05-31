//! `FileContext` implementation for [`crate::Window`].
//!
//! File commands are separate from view/theme commands so the command
//! crate's trait files stay small. Every method runs on the owning UI
//! thread; disk reads and directory enumeration are delegated to the
//! file-I/O worker.

use crate::window::Window;
use crate::window_view_context::map_ui_to_command_error;

impl continuity_command::FileContext for Window {
    fn file_open_dialog(&mut self) -> Result<(), continuity_command::Error> {
        self.file_open_dialog_impl()
    }

    fn file_open_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
    ) -> Result<(), continuity_command::Error> {
        self.file_open_paths_impl(paths)
    }

    fn file_open_folder(
        &mut self,
        path: Option<std::path::PathBuf>,
    ) -> Result<(), continuity_command::Error> {
        self.file_open_folder_impl(path)
    }

    fn toggle_file_tree(&mut self) -> Result<(), continuity_command::Error> {
        self.toggle_file_tree_impl()
            .map_err(map_ui_to_command_error)
    }

    fn file_save(&mut self) -> Result<(), continuity_command::Error> {
        self.file_save_impl()
    }

    fn file_save_as(&mut self) -> Result<(), continuity_command::Error> {
        self.file_save_as_impl()
    }

    fn file_reload_external(&mut self) -> Result<(), continuity_command::Error> {
        self.file_reload_external_impl()
    }

    fn file_keep_mine(&mut self) -> Result<(), continuity_command::Error> {
        self.file_keep_mine_impl()
    }

    fn file_show_diff(&mut self) -> Result<(), continuity_command::Error> {
        self.file_show_diff_impl()
    }
}
