//! Startup file adoption for `Open with` / command-line paths.
//!
//! The app crate reads those files before window threads spawn and asks
//! core to create file-associated buffers. This UI-thread helper only
//! installs the resulting buffer ids into the first restored window's
//! focused pane, so startup opens augment session restore instead of
//! replacing it.

use continuity_buffer::BufferId;
use std::path::PathBuf;

use crate::window::Window;

impl Window {
    /// Adopt every process-startup file buffer as a new tab.
    ///
    /// Thread ownership: mutates this window's pane tree on its UI
    /// thread. Buffer contents and file associations are already owned by
    /// the core thread.
    pub(crate) fn adopt_startup_open_buffers(&mut self, buffer_ids: Vec<BufferId>) {
        let had_buffers = !buffer_ids.is_empty();
        for buffer_id in buffer_ids {
            self.adopt_buffer_as_new_tab(buffer_id);
        }
        if had_buffers {
            self.save_window_placement_state();
        }
    }

    /// Open the first process-startup folder in the file-tree pane.
    pub(crate) fn adopt_startup_folder_roots(&mut self, roots: Vec<PathBuf>) {
        let Some(root) = roots.into_iter().next() else {
            return;
        };
        let _ = self.open_folder_root(root);
    }
}
