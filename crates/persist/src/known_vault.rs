//! Machine-local registry of vault roots used by the vault launcher.

use std::path::{Path, PathBuf};

use crossbeam_channel::bounded;

use crate::{Error, PersistClient, PersistMessage};

/// One vault shown in the launcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnownVault {
    /// Canonical vault root.
    pub root_path: PathBuf,
    /// Human-readable root directory name.
    pub display_name: String,
    /// Whether the vault stays above unpinned recent entries.
    pub pinned: bool,
    /// Most recent successful vault activation, as Unix milliseconds.
    pub last_opened_ms: i64,
}

impl PersistClient {
    /// Insert or refresh one known vault.
    pub fn upsert_known_vault(&self, vault: KnownVault) -> Result<(), Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::UpsertKnownVault { vault, reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// List pinned and recent vaults in launcher order.
    pub fn list_known_vaults(&self) -> Result<Vec<KnownVault>, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::ListKnownVaults { reply: tx })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Pin or unpin a known vault. Returns whether a row changed.
    pub fn set_known_vault_pinned(&self, root_path: &Path, pinned: bool) -> Result<bool, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::SetKnownVaultPinned {
                root_path: root_path.to_path_buf(),
                pinned,
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }

    /// Forget a vault without changing anything inside it.
    pub fn remove_known_vault(&self, root_path: &Path) -> Result<bool, Error> {
        let (tx, rx) = bounded(1);
        self.sender()
            .send(PersistMessage::RemoveKnownVault {
                root_path: root_path.to_path_buf(),
                reply: tx,
            })
            .map_err(|_| Error::ThreadGone)?;
        rx.recv().map_err(|_| Error::ThreadGone)?
    }
}
