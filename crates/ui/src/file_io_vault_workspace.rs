//! File-I/O-worker primitives for portable vault workspace state.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use continuity_config::{VaultWorkspaceState, VAULT_CONFIG_DIRECTORY, VAULT_WORKSPACE_FILE};

pub(crate) fn read_vault_workspace(root: &Path) -> io::Result<VaultWorkspaceState> {
    let path = workspace_path(root);
    match std::fs::read_to_string(&path) {
        Ok(source) => VaultWorkspaceState::from_toml_validated(&source)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(VaultWorkspaceState::default()),
        Err(error) => Err(error),
    }
}

pub(crate) fn write_vault_workspace(root: &Path, state: &VaultWorkspaceState) -> io::Result<()> {
    let body = state
        .to_toml()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let path = workspace_path(root);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "workspace path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("toml.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(temporary, path)
}

fn workspace_path(root: &Path) -> PathBuf {
    root.join(VAULT_CONFIG_DIRECTORY).join(VAULT_WORKSPACE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_workspace_uses_defaults_and_write_round_trips() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            read_vault_workspace(directory.path()).expect("default workspace"),
            VaultWorkspaceState::default()
        );
        let state = VaultWorkspaceState {
            file_tree_width_dip: 365.0,
            file_tree_visible: false,
            expanded_directories: vec!["notes/daily".into()],
            ..VaultWorkspaceState::default()
        };
        write_vault_workspace(directory.path(), &state).expect("write workspace");
        assert_eq!(
            read_vault_workspace(directory.path()).expect("read workspace"),
            state
        );
    }

    #[test]
    fn invalid_workspace_is_reported() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config_directory = directory.path().join(VAULT_CONFIG_DIRECTORY);
        std::fs::create_dir_all(&config_directory).expect("config directory");
        std::fs::write(config_directory.join(VAULT_WORKSPACE_FILE), "version = 9")
            .expect("invalid workspace");
        assert_eq!(
            read_vault_workspace(directory.path())
                .expect_err("invalid workspace rejected")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
