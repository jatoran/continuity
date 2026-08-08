//! UI-thread snapshot dispatch for portable vault workspace state.

use std::path::PathBuf;

use crate::window::Window;
use crate::window_file::FileBanner;

impl Window {
    pub(crate) fn persist_vault_workspace_state(&mut self) {
        let Some(root) = self.vault.root().map(PathBuf::from) else {
            return;
        };
        // A restore in flight has not yet installed every tab; persisting now
        // would truncate the saved set to whatever has landed so far.
        if self.vault.pending_tab_restore.is_some() {
            return;
        }
        let (open_tabs, focused_tab) = self.collect_vault_open_tabs();
        let mut state = self.file_tree.vault_workspace_state();
        state.open_tabs = open_tabs;
        state.focused_tab = focused_tab;
        let Some(file_io) = self.file_io.as_ref() else {
            return;
        };
        if !file_io.persist_vault_workspace(root, state, self.file_open_tx.clone()) {
            self.file_banner = Some(FileBanner::new(
                "Vault workspace persistence is unavailable".into(),
            ));
        }
    }
}
