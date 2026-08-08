//! Vault-specific request methods on the shared file-I/O client.

use std::path::PathBuf;

use crossbeam_channel::Sender;

use crate::file_io::{FileIoClient, FileIoEvent, FileIoRequest, VaultEntryKind};

impl FileIoClient {
    /// Canonicalize a folder and find its nearest vault marker.
    pub(crate) fn inspect_folder(&self, path: PathBuf, reply: Sender<FileIoEvent>) -> bool {
        self.tx
            .send(FileIoRequest::InspectFolder { path, reply })
            .is_ok()
    }

    /// Create a default vault marker and return the validated vault.
    pub(crate) fn initialize_vault(&self, path: PathBuf, reply: Sender<FileIoEvent>) -> bool {
        self.tx
            .send(FileIoRequest::InitializeVault { path, reply })
            .is_ok()
    }

    pub(crate) fn create_vault_entry(
        &self,
        root: PathBuf,
        parent: PathBuf,
        kind: VaultEntryKind,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::CreateVaultEntry {
                root,
                parent,
                kind,
                reply,
            })
            .is_ok()
    }

    pub(crate) fn move_vault_entry(
        &self,
        root: PathBuf,
        source: PathBuf,
        destination_directory: PathBuf,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::MoveVaultEntry {
                root,
                source,
                destination_directory,
                reply,
            })
            .is_ok()
    }

    pub(crate) fn rename_tree_entry(
        &self,
        root: PathBuf,
        source: PathBuf,
        new_name: String,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::RenameTreeEntry {
                root,
                source,
                new_name,
                reply,
            })
            .is_ok()
    }

    pub(crate) fn delete_vault_entry(
        &self,
        root: PathBuf,
        relative: PathBuf,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::DeleteVaultEntry {
                root,
                relative,
                reply,
            })
            .is_ok()
    }

    pub(crate) fn persist_vault_workspace(
        &self,
        root: PathBuf,
        state: continuity_config::VaultWorkspaceState,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::PersistVaultWorkspace { root, state, reply })
            .is_ok()
    }

    pub(crate) fn list_directory(
        &self,
        root: PathBuf,
        relative: PathBuf,
        config: Option<continuity_config::VaultConfig>,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::ListDirectory {
                root,
                relative,
                config,
                reply,
            })
            .is_ok()
    }
}
