//! Window integration for the known-vault launcher and shortcuts.

use std::path::{Path, PathBuf};

use crate::window_file::FileBanner;
use crate::window_file_dialogs::open_folder_dialog;
use crate::Window;

impl Window {
    pub(crate) fn open_vault_launcher_impl(&mut self) {
        let vaults = self
            .persist_client
            .as_ref()
            .and_then(|client| client.list_known_vaults().ok())
            .unwrap_or_default();
        self.overlays.open_vault_launcher(vaults);
        self.focus_overlay_input();
    }

    pub(crate) fn confirm_vault_launcher(&mut self, open_here: bool) {
        let action = self
            .overlays
            .vault_launcher_mut()
            .and_then(|launcher| launcher.selected_action());
        match action {
            Some(crate::vault_launcher::VaultLauncherAction::Browse) => {
                self.browse_vault_from_launcher();
                return;
            }
            Some(crate::vault_launcher::VaultLauncherAction::Initialize) => {
                self.initialize_vault_from_launcher();
                return;
            }
            None => {}
        }
        let selected = self
            .overlays
            .vault_launcher_mut()
            .and_then(|launcher| launcher.selected_vault().cloned());
        let Some(vault) = selected else {
            return;
        };
        self.dismiss_overlay_and_blur();
        if !open_here && self.vault.open_in_window(vault.root_path.clone()) {
            return;
        }
        let _ = self.open_folder_root(vault.root_path);
    }

    pub(crate) fn toggle_selected_vault_pin(&mut self) {
        let selected = self
            .overlays
            .vault_launcher_mut()
            .and_then(|launcher| launcher.selected_vault().cloned());
        let (Some(client), Some(vault)) = (self.persist_client.as_ref(), selected) else {
            return;
        };
        let _ = client.set_known_vault_pinned(&vault.root_path, !vault.pinned);
        self.open_vault_launcher_impl();
    }

    pub(crate) fn forget_selected_vault(&mut self) {
        let selected = self
            .overlays
            .vault_launcher_mut()
            .and_then(|launcher| launcher.selected_vault().cloned());
        let (Some(client), Some(vault)) = (self.persist_client.as_ref(), selected) else {
            return;
        };
        let _ = client.remove_known_vault(&vault.root_path);
        self.open_vault_launcher_impl();
    }

    pub(crate) fn shortcut_selected_vault(&mut self) {
        let root = self
            .overlays
            .vault_launcher_mut()
            .and_then(|launcher| launcher.selected_vault())
            .map(|vault| vault.root_path.clone());
        if let Some(root) = root {
            self.create_vault_shortcut(&root);
        }
    }

    pub(crate) fn create_current_vault_shortcut(&mut self) {
        if let Some(root) = self.vault.root().map(PathBuf::from) {
            self.create_vault_shortcut(&root);
        }
    }

    fn create_vault_shortcut(&mut self, root: &Path) {
        let now_ms = self.now_ms();
        let message = match crate::windows_shortcut::create_vault_desktop_shortcut(root) {
            Ok(path) => format!("Created desktop shortcut {}", path.display()),
            Err(error) => format!("Desktop shortcut failed: {error}"),
        };
        self.file_banner = Some(FileBanner::transient(message, now_ms));
    }

    pub(crate) fn browse_vault_from_launcher(&mut self) {
        let Some(root) = open_folder_dialog(self.hwnd) else {
            return;
        };
        self.dismiss_overlay_and_blur();
        let _ = self.open_folder_root(root);
    }

    pub(crate) fn initialize_vault_from_launcher(&mut self) {
        let Some(root) = open_folder_dialog(self.hwnd) else {
            return;
        };
        let Some(file_io) = self.file_io.as_ref() else {
            return;
        };
        if file_io.initialize_vault(root.clone(), self.file_open_tx.clone()) {
            self.dismiss_overlay_and_blur();
            self.file_banner = Some(FileBanner::new(format!(
                "Initializing vault at {}…",
                root.display()
            )));
        }
    }
}
