//! File-I/O worker loop.
//!
//! Split from [`crate::file_io`] so request/event type definitions stay
//! compact. This thread owns filesystem reads, writes, watches, and
//! bounded directory listing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use continuity_buffer::{BufferId, FileAssociation};
use crossbeam_channel::{bounded, select, Receiver, Sender};
use notify::{Event, RecommendedWatcher};

use crate::file_io::{FileIoEvent, FileIoRequest};
use crate::file_io_directory::read_directory;
use crate::file_io_primitives::{
    install_watch, is_self_write, normalize_path, read_file, send_failed, write_file,
    ReadFileResult,
};
use crate::file_io_vault_entries::{create_entry, move_entry, recycle_entry, rename_entry};
use crate::file_io_vault_workspace::write_vault_workspace;
use crate::file_io_worker_vault::{
    handle_vault_notify, register_vault_subscriber, send_vault_change, send_vault_inspection,
};

pub(crate) const CHANNEL_CAPACITY: usize = 1024;

/// Watched file metadata owned by the file-I/O worker.
#[derive(Clone)]
pub(crate) struct WatchedFile {
    pub(crate) buffer_id: BufferId,
    pub(crate) file: FileAssociation,
}

/// Vault marker routing owned by the file-I/O worker thread.
pub(crate) struct WatchedVault {
    pub(crate) root: PathBuf,
    pub(crate) subscribers: Vec<Sender<FileIoEvent>>,
}

pub(crate) fn worker_loop(rx: Receiver<FileIoRequest>, event_tx: Sender<FileIoEvent>) {
    let (notify_tx, notify_rx) = bounded::<notify::Result<Event>>(CHANNEL_CAPACITY);
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = notify_tx.send(res);
    }) {
        Ok(w) => Some(w),
        Err(e) => {
            let _ = event_tx.send(FileIoEvent::Failed {
                buffer_id: None,
                operation: "watch",
                path: None,
                reason: e.to_string(),
            });
            None
        }
    };
    let mut watched: HashMap<PathBuf, WatchedFile> = HashMap::new();
    let mut watched_dirs: HashSet<PathBuf> = HashSet::new();
    let mut watched_vaults: HashMap<PathBuf, WatchedVault> = HashMap::new();
    loop {
        select! {
            recv(rx) -> msg => {
                let Ok(msg) = msg else { break; };
                if handle_request(msg, &event_tx, watcher.as_mut(), &mut watched, &mut watched_dirs, &mut watched_vaults) {
                    break;
                }
            }
            recv(notify_rx) -> msg => {
                if let Ok(res) = msg {
                    handle_vault_notify(&res, &event_tx, &watched_vaults);
                    handle_notify(res, &event_tx, &mut watched);
                }
            }
        }
    }
}

fn handle_request(
    msg: FileIoRequest,
    event_tx: &Sender<FileIoEvent>,
    mut watcher: Option<&mut RecommendedWatcher>,
    watched: &mut HashMap<PathBuf, WatchedFile>,
    watched_dirs: &mut HashSet<PathBuf>,
    watched_vaults: &mut HashMap<PathBuf, WatchedVault>,
) -> bool {
    match msg {
        FileIoRequest::InspectFolder { path, reply } => {
            if let Some((marker, root)) = send_vault_inspection(&reply, "open folder", path, false)
            {
                install_watch(watcher, watched_dirs, &marker);
                register_vault_subscriber(watched_vaults, marker, root, reply);
            }
            false
        }
        FileIoRequest::InitializeVault { path, reply } => {
            if let Some((marker, root)) =
                send_vault_inspection(&reply, "initialize vault", path, true)
            {
                install_watch(watcher, watched_dirs, &marker);
                register_vault_subscriber(watched_vaults, marker, root, reply);
            }
            false
        }
        FileIoRequest::CreateVaultEntry {
            root,
            parent,
            kind,
            reply,
        } => {
            match create_entry(&root, &parent, kind) {
                Ok(_) => {
                    let changed = FileIoEvent::VaultEntriesChanged {
                        root: root.clone(),
                        refresh_relative: parent,
                        moved: None,
                        deleted: None,
                    };
                    send_vault_change(watched_vaults, &root, &reply, changed);
                }
                Err(error) => send_failed(&reply, "create vault entry", None, Some(root), error),
            }
            false
        }
        FileIoRequest::MoveVaultEntry {
            root,
            source,
            destination_directory,
            reply,
        } => {
            match move_entry(&root, &source, &destination_directory) {
                Ok(moved) => {
                    let source_absolute = normalize_path(&root.join(&moved.0));
                    let destination_absolute = normalize_path(&root.join(&moved.1));
                    let moved_watch_paths =
                        reassociate_watched_paths(watched, &source_absolute, &destination_absolute);
                    for moved_path in moved_watch_paths {
                        install_watch(watcher.as_deref_mut(), watched_dirs, &moved_path);
                    }
                    let refresh_relative = source
                        .parent()
                        .unwrap_or(std::path::Path::new(""))
                        .to_path_buf();
                    let changed = FileIoEvent::VaultEntriesChanged {
                        root: root.clone(),
                        refresh_relative,
                        moved: Some(moved),
                        deleted: None,
                    };
                    send_vault_change(watched_vaults, &root, &reply, changed);
                }
                Err(error) => send_failed(&reply, "move vault entry", None, Some(root), error),
            }
            false
        }
        FileIoRequest::RenameTreeEntry {
            root,
            source,
            new_name,
            reply,
        } => {
            match rename_entry(&root, &source, &new_name) {
                Ok(moved) => {
                    let source_absolute = normalize_path(&root.join(&moved.0));
                    let destination_absolute = normalize_path(&root.join(&moved.1));
                    let moved_watch_paths =
                        reassociate_watched_paths(watched, &source_absolute, &destination_absolute);
                    for moved_path in moved_watch_paths {
                        install_watch(watcher.as_deref_mut(), watched_dirs, &moved_path);
                    }
                    let refresh_relative = source
                        .parent()
                        .unwrap_or(std::path::Path::new(""))
                        .to_path_buf();
                    let changed = FileIoEvent::VaultEntriesChanged {
                        root: root.clone(),
                        refresh_relative,
                        moved: Some(moved),
                        deleted: None,
                    };
                    send_vault_change(watched_vaults, &root, &reply, changed);
                }
                Err(error) => send_failed(&reply, "rename tree entry", None, Some(root), error),
            }
            false
        }
        FileIoRequest::DeleteVaultEntry {
            root,
            relative,
            reply,
        } => {
            match recycle_entry(&root, &relative) {
                Ok(deleted) => {
                    let refresh_relative = relative
                        .parent()
                        .unwrap_or(std::path::Path::new(""))
                        .to_path_buf();
                    let changed = FileIoEvent::VaultEntriesChanged {
                        root: root.clone(),
                        refresh_relative,
                        moved: None,
                        deleted: Some(deleted),
                    };
                    send_vault_change(watched_vaults, &root, &reply, changed);
                }
                Err(error) => send_failed(&reply, "recycle vault entry", None, Some(root), error),
            }
            false
        }
        FileIoRequest::PersistVaultWorkspace { root, state, reply } => {
            if let Err(error) = write_vault_workspace(&root, &state) {
                send_failed(&reply, "persist vault workspace", None, Some(root), error);
            }
            false
        }
        FileIoRequest::OpenFiles {
            paths,
            target_pane,
            reply,
            wake_window,
            disposition,
        } => {
            let output = reply.as_ref().unwrap_or(event_tx);
            for path in paths {
                match read_file(&path) {
                    Ok(result) => {
                        let ReadFileResult {
                            content,
                            file,
                            encoding_notice,
                        } = result;
                        if let Some(encoding) = encoding_notice {
                            let _ = output.send(FileIoEvent::EncodingNotice {
                                path: path.clone(),
                                encoding,
                            });
                        }
                        let _ = output.send(FileIoEvent::Opened {
                            target_pane,
                            content,
                            file,
                            disposition,
                        });
                    }
                    Err(e) => send_failed(output, "open", None, Some(path), e),
                }
            }
            crate::file_io_wake::wake_file_io_window(wake_window);
            false
        }
        FileIoRequest::ListDirectory {
            root,
            relative,
            config,
            reply,
        } => {
            match read_directory(&root, &relative, config.as_ref()) {
                Ok(listing) => {
                    let _ = reply.send(FileIoEvent::DirectoryListed {
                        root: listing.root,
                        relative: listing.relative,
                        entries: listing.entries,
                        truncated: listing.truncated,
                    });
                }
                Err(e) => {
                    let path = root.join(&relative);
                    let reason = e.to_string();
                    let _ = reply.send(FileIoEvent::DirectoryListed {
                        root,
                        relative,
                        entries: Vec::new(),
                        truncated: false,
                    });
                    let _ = reply.send(FileIoEvent::Failed {
                        buffer_id: None,
                        operation: "list folder",
                        path: Some(path),
                        reason,
                    });
                }
            }
            false
        }
        FileIoRequest::SaveBuffer {
            buffer_id,
            path,
            content,
            expected_hash,
            reply,
        } => {
            // Route completion to the requesting window so a sibling window
            // draining the shared event channel cannot steal it (which would
            // wedge the owning window's autosave `in_flight` forever).
            let output = reply.as_ref().unwrap_or(event_tx);
            // Conflict guard: when an expected fingerprint is given, re-read
            // the file and refuse the save if its raw hash changed since the
            // buffer last synced — overwriting would silently destroy an
            // external edit. A missing/unreadable file is not a conflict
            // (the write recreates it). This closes the race where a save
            // beats the asynchronous `notify` watcher.
            let conflict = match expected_hash {
                Some(expected) => match read_file(&path) {
                    Ok(current) if current.file.hash != expected => Some(current),
                    _ => None,
                },
                None => None,
            };
            if let Some(current) = conflict {
                let _ = output.send(FileIoEvent::SaveConflict {
                    buffer_id,
                    path,
                    content: current.content,
                    file: current.file,
                });
            } else {
                match write_file(&path, &content) {
                    Ok(file) => {
                        install_watch(watcher, watched_dirs, &path);
                        watched.insert(
                            normalize_path(&path),
                            WatchedFile {
                                buffer_id,
                                file: file.clone(),
                            },
                        );
                        let _ = output.send(FileIoEvent::Saved { buffer_id, file });
                    }
                    Err(e) => send_failed(output, "save", Some(buffer_id), Some(path), e),
                }
            }
            false
        }
        FileIoRequest::ReloadBuffer { buffer_id, path } => {
            match read_file(&path) {
                Ok(result) => {
                    let ReadFileResult {
                        content,
                        file,
                        encoding_notice,
                    } = result;
                    install_watch(watcher, watched_dirs, &path);
                    watched.insert(
                        normalize_path(&path),
                        WatchedFile {
                            buffer_id,
                            file: file.clone(),
                        },
                    );
                    if let Some(encoding) = encoding_notice {
                        let _ = event_tx.send(FileIoEvent::EncodingNotice {
                            path: path.clone(),
                            encoding,
                        });
                    }
                    let _ = event_tx.send(FileIoEvent::Reloaded {
                        buffer_id,
                        content,
                        file,
                    });
                }
                Err(e) => send_failed(event_tx, "reload", Some(buffer_id), Some(path), e),
            }
            false
        }
        FileIoRequest::RecheckFile {
            buffer_id,
            path,
            reply,
        } => {
            let output = reply.as_ref().unwrap_or(event_tx);
            // A missing/unreadable file on recheck is intentionally silent:
            // the rope stays canonical (the file is just an export) and a
            // later save recreates the path — no banner.
            if let Ok(result) = read_file(&path) {
                let ReadFileResult {
                    content,
                    file,
                    encoding_notice,
                } = result;
                // Arm/refresh the watch so a restored buffer that was never
                // watched this session begins observing future edits.
                install_watch(watcher, watched_dirs, &path);
                watched.insert(
                    normalize_path(&path),
                    WatchedFile {
                        buffer_id,
                        file: file.clone(),
                    },
                );
                if let Some(encoding) = encoding_notice {
                    let _ = output.send(FileIoEvent::EncodingNotice {
                        path: path.clone(),
                        encoding,
                    });
                }
                let _ = output.send(FileIoEvent::Rechecked {
                    buffer_id,
                    content,
                    file,
                });
            }
            false
        }
        FileIoRequest::WatchFile { buffer_id, file } => {
            install_watch(watcher, watched_dirs, &file.path);
            watched.insert(normalize_path(&file.path), WatchedFile { buffer_id, file });
            false
        }
        FileIoRequest::Shutdown => true,
    }
}

fn reassociate_watched_paths(
    watched: &mut HashMap<PathBuf, WatchedFile>,
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Vec<PathBuf> {
    let updates: Vec<_> = watched
        .iter()
        .filter_map(|(path, entry)| {
            path.strip_prefix(source).ok().map(|suffix| {
                let mut entry = entry.clone();
                let next = destination.join(suffix);
                entry.file.path = next.clone();
                (path.clone(), next, entry)
            })
        })
        .collect();
    let mut moved_paths = Vec::with_capacity(updates.len());
    for (old, next, entry) in updates {
        watched.remove(&old);
        moved_paths.push(next.clone());
        watched.insert(next, entry);
    }
    moved_paths
}

#[cfg(test)]
pub(crate) fn handle_notify(
    res: notify::Result<Event>,
    event_tx: &Sender<FileIoEvent>,
    watched: &mut HashMap<PathBuf, WatchedFile>,
) {
    handle_notify_inner(res, event_tx, watched);
}

#[cfg(not(test))]
fn handle_notify(
    res: notify::Result<Event>,
    event_tx: &Sender<FileIoEvent>,
    watched: &mut HashMap<PathBuf, WatchedFile>,
) {
    handle_notify_inner(res, event_tx, watched);
}

fn handle_notify_inner(
    res: notify::Result<Event>,
    event_tx: &Sender<FileIoEvent>,
    watched: &mut HashMap<PathBuf, WatchedFile>,
) {
    let Ok(event) = res else {
        return;
    };
    for path in event.paths {
        let key = normalize_path(&path);
        let Some(watched_file) = watched.get(&key).cloned() else {
            continue;
        };
        match read_file(&path) {
            Ok(result) => {
                let ReadFileResult {
                    content,
                    file: observed,
                    encoding_notice,
                } = result;
                if is_self_write(&observed, &watched_file.file) {
                    continue;
                }
                watched.insert(
                    key,
                    WatchedFile {
                        buffer_id: watched_file.buffer_id,
                        file: observed.clone(),
                    },
                );
                if let Some(encoding) = encoding_notice {
                    let _ = event_tx.send(FileIoEvent::EncodingNotice {
                        path: path.clone(),
                        encoding,
                    });
                }
                let _ = event_tx.send(FileIoEvent::ExternalChanged {
                    buffer_id: watched_file.buffer_id,
                    path,
                    content,
                    file: observed,
                });
            }
            Err(_) => {
                if !path.exists() {
                    watched.remove(&key);
                    let _ = event_tx.send(FileIoEvent::Deleted {
                        buffer_id: watched_file.buffer_id,
                        path,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "file_io_worker_save_tests.rs"]
mod save_tests;

#[cfg(test)]
#[path = "file_io_worker_open_tests.rs"]
mod open_tests;
