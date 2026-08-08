//! Vault inspection, config-watch, and subscriber routing for file I/O.

use std::collections::HashMap;
use std::path::PathBuf;

use continuity_config::{VaultConfig, VAULT_CONFIG_DIRECTORY, VAULT_CONFIG_FILE};
use crossbeam_channel::Sender;
use notify::Event;

use crate::file_io::FileIoEvent;
use crate::file_io_primitives::{normalize_path, send_failed};
use crate::file_io_vault::{initialize_vault, inspect_vault};
use crate::file_io_worker::WatchedVault;

pub(crate) fn send_vault_inspection(
    output: &Sender<FileIoEvent>,
    operation: &'static str,
    path: PathBuf,
    should_initialize: bool,
) -> Option<(PathBuf, PathBuf)> {
    let result = if should_initialize {
        initialize_vault(&path)
    } else {
        inspect_vault(&path)
    };
    match result {
        Ok(inspection) => {
            let watch = inspection.config.as_ref().map(|_| {
                (
                    inspection
                        .root
                        .join(VAULT_CONFIG_DIRECTORY)
                        .join(VAULT_CONFIG_FILE),
                    inspection.root.clone(),
                )
            });
            let warning_root = inspection.root.clone();
            let warning = inspection.workspace_warning;
            let _ = output.send(FileIoEvent::FolderInspected {
                requested_root: inspection.requested_root,
                root: inspection.root,
                config: inspection.config,
                workspace: inspection.workspace,
            });
            if let Some(reason) = warning {
                let _ = output.send(FileIoEvent::Failed {
                    buffer_id: None,
                    operation: "load vault workspace",
                    path: Some(
                        warning_root
                            .join(VAULT_CONFIG_DIRECTORY)
                            .join(continuity_config::VAULT_WORKSPACE_FILE),
                    ),
                    reason,
                });
            }
            watch
        }
        Err(error) => {
            send_failed(output, operation, None, Some(path), error);
            None
        }
    }
}

pub(crate) fn handle_vault_notify(
    event: &notify::Result<Event>,
    output: &Sender<FileIoEvent>,
    watched_vaults: &HashMap<PathBuf, WatchedVault>,
) {
    let Ok(event) = event else {
        return;
    };
    for path in &event.paths {
        let normalized = normalize_path(path);
        let Some(vault) = watched_vaults.get(&normalized) else {
            continue;
        };
        match std::fs::read_to_string(path)
            .map_err(std::io::Error::other)
            .and_then(|source| {
                VaultConfig::from_toml_validated(&source).map_err(std::io::Error::other)
            }) {
            Ok(config) => fan_out(
                output,
                vault,
                FileIoEvent::VaultConfigChanged {
                    root: vault.root.clone(),
                    config,
                },
            ),
            Err(error) => fan_out(
                output,
                vault,
                FileIoEvent::Failed {
                    buffer_id: None,
                    operation: "reload vault config",
                    path: Some(path.clone()),
                    reason: error.to_string(),
                },
            ),
        }
    }
}

pub(crate) fn register_vault_subscriber(
    watched_vaults: &mut HashMap<PathBuf, WatchedVault>,
    marker: PathBuf,
    root: PathBuf,
    reply: Sender<FileIoEvent>,
) {
    let watched = watched_vaults
        .entry(normalize_path(&marker))
        .or_insert_with(|| WatchedVault {
            root,
            subscribers: Vec::new(),
        });
    if !watched
        .subscribers
        .iter()
        .any(|subscriber| subscriber.same_channel(&reply))
    {
        watched.subscribers.push(reply);
    }
}

pub(crate) fn send_vault_change(
    watched_vaults: &HashMap<PathBuf, WatchedVault>,
    root: &std::path::Path,
    reply: &Sender<FileIoEvent>,
    event: FileIoEvent,
) {
    let mut sent_to_reply = false;
    for vault in watched_vaults.values().filter(|vault| vault.root == root) {
        for subscriber in &vault.subscribers {
            sent_to_reply |= subscriber.same_channel(reply);
            let _ = subscriber.send(event.clone());
        }
    }
    if !sent_to_reply {
        let _ = reply.send(event);
    }
}

fn fan_out(output: &Sender<FileIoEvent>, vault: &WatchedVault, event: FileIoEvent) {
    if vault.subscribers.is_empty() {
        let _ = output.send(event);
    } else {
        for subscriber in &vault.subscribers {
            let _ = subscriber.send(event.clone());
        }
    }
}
