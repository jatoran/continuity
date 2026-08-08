//! Vault discovery and initialization owned by the file-I/O worker.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use continuity_config::{
    VaultConfig, VaultWorkspaceState, DEFAULT_VAULT_TOML, VAULT_CONFIG_DIRECTORY, VAULT_CONFIG_FILE,
};

use crate::file_io_vault_workspace::{read_vault_workspace, write_vault_workspace};

/// Canonical folder and its nearest vault, when one exists.
pub(crate) struct VaultInspection {
    pub(crate) requested_root: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) config: Option<VaultConfig>,
    pub(crate) workspace: Option<VaultWorkspaceState>,
    pub(crate) workspace_warning: Option<String>,
}

pub(crate) fn inspect_vault(path: &Path) -> io::Result<VaultInspection> {
    let requested_root = path.canonicalize()?;
    if !requested_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a folder",
        ));
    }
    for ancestor in requested_root.ancestors() {
        let marker = marker_path(ancestor);
        if marker.is_file() {
            let source = std::fs::read_to_string(&marker)?;
            let config = VaultConfig::from_toml_validated(&source).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", marker.display()),
                )
            })?;
            let root = ancestor.to_path_buf();
            let (workspace, workspace_warning) = match read_vault_workspace(&root) {
                Ok(workspace) => (workspace, None),
                Err(error) => (VaultWorkspaceState::default(), Some(error.to_string())),
            };
            return Ok(VaultInspection {
                requested_root,
                root,
                config: Some(config),
                workspace: Some(workspace),
                workspace_warning,
            });
        }
    }
    Ok(VaultInspection {
        root: requested_root.clone(),
        requested_root,
        config: None,
        workspace: None,
        workspace_warning: None,
    })
}

pub(crate) fn initialize_vault(path: &Path) -> io::Result<VaultInspection> {
    let root = path.canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a folder",
        ));
    }
    let config_directory = root.join(VAULT_CONFIG_DIRECTORY);
    std::fs::create_dir_all(&config_directory)?;
    let marker = config_directory.join(VAULT_CONFIG_FILE);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            file.write_all(DEFAULT_VAULT_TOML.as_bytes())?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let workspace_path = config_directory.join(continuity_config::VAULT_WORKSPACE_FILE);
    if !workspace_path.exists() {
        write_vault_workspace(&root, &VaultWorkspaceState::default())?;
    }
    inspect_vault(&root)
}

fn marker_path(root: &Path) -> PathBuf {
    root.join(VAULT_CONFIG_DIRECTORY).join(VAULT_CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_creates_marker_and_nearest_ancestor_wins() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let nested = directory.path().join("notes").join("daily");
        std::fs::create_dir_all(&nested).expect("nested directory");
        let initialized = initialize_vault(directory.path()).expect("initialize vault");
        assert_eq!(
            initialized.root,
            directory.path().canonicalize().expect("root")
        );
        assert!(initialized.config.is_some());
        assert!(initialized.workspace.is_some());
        assert!(directory
            .path()
            .join(VAULT_CONFIG_DIRECTORY)
            .join(continuity_config::VAULT_WORKSPACE_FILE)
            .is_file());

        let discovered = inspect_vault(&nested).expect("inspect nested path");
        assert_eq!(discovered.root, initialized.root);
        assert_eq!(
            discovered.requested_root,
            nested.canonicalize().expect("nested")
        );
    }

    #[test]
    fn config_directory_without_marker_is_not_a_vault() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir(directory.path().join(VAULT_CONFIG_DIRECTORY))
            .expect("config directory");
        let inspected = inspect_vault(directory.path()).expect("inspect folder");
        assert!(inspected.config.is_none());
        assert!(inspected.workspace.is_none());
    }
}
