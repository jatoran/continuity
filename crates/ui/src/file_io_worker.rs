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

pub(crate) const CHANNEL_CAPACITY: usize = 1024;

/// Watched file metadata owned by the file-I/O worker.
#[derive(Clone)]
pub(crate) struct WatchedFile {
    pub(crate) buffer_id: BufferId,
    pub(crate) file: FileAssociation,
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
    loop {
        select! {
            recv(rx) -> msg => {
                let Ok(msg) = msg else { break; };
                if handle_request(msg, &event_tx, watcher.as_mut(), &mut watched, &mut watched_dirs) {
                    break;
                }
            }
            recv(notify_rx) -> msg => {
                if let Ok(res) = msg {
                    handle_notify(res, &event_tx, &mut watched);
                }
            }
        }
    }
}

fn handle_request(
    msg: FileIoRequest,
    event_tx: &Sender<FileIoEvent>,
    watcher: Option<&mut RecommendedWatcher>,
    watched: &mut HashMap<PathBuf, WatchedFile>,
    watched_dirs: &mut HashSet<PathBuf>,
) -> bool {
    match msg {
        FileIoRequest::OpenFiles {
            paths,
            target_pane,
            reply,
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
                        });
                    }
                    Err(e) => send_failed(output, "open", None, Some(path), e),
                }
            }
            false
        }
        FileIoRequest::ListDirectory { root, relative } => {
            match read_directory(&root, &relative) {
                Ok(listing) => {
                    let _ = event_tx.send(FileIoEvent::DirectoryListed {
                        root: listing.root,
                        relative: listing.relative,
                        entries: listing.entries,
                        truncated: listing.truncated,
                    });
                }
                Err(e) => {
                    let path = root.join(&relative);
                    let reason = e.to_string();
                    let _ = event_tx.send(FileIoEvent::DirectoryListed {
                        root,
                        relative,
                        entries: Vec::new(),
                        truncated: false,
                    });
                    let _ = event_tx.send(FileIoEvent::Failed {
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
        } => {
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
                let _ = event_tx.send(FileIoEvent::SaveConflict {
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
                        let _ = event_tx.send(FileIoEvent::Saved { buffer_id, file });
                    }
                    Err(e) => send_failed(event_tx, "save", Some(buffer_id), Some(path), e),
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
mod tests {
    use std::collections::{HashMap, HashSet};

    use crossbeam_channel::bounded;

    use super::*;

    #[test]
    fn open_files_with_reply_uses_reply_channel() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("note.md");
        std::fs::write(&path, "hello").expect("write temp file");
        let (event_tx, event_rx) = bounded(CHANNEL_CAPACITY);
        let (reply_tx, reply_rx) = bounded(CHANNEL_CAPACITY);
        let mut watched = HashMap::new();
        let mut watched_dirs = HashSet::new();

        let should_shutdown = handle_request(
            FileIoRequest::OpenFiles {
                paths: vec![path.clone()],
                target_pane: None,
                reply: Some(reply_tx),
            },
            &event_tx,
            None,
            &mut watched,
            &mut watched_dirs,
        );

        assert!(!should_shutdown);
        assert!(event_rx.try_recv().is_err());
        let event = reply_rx.try_recv().expect("receive open reply");
        let FileIoEvent::Opened {
            target_pane,
            content,
            file,
        } = event
        else {
            panic!("expected opened event");
        };
        assert_eq!(target_pane, None);
        assert_eq!(content, "hello");
        assert_eq!(file.path, path);
    }

    #[test]
    fn recheck_file_replies_with_current_disk_bytes_and_arms_watch() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("restored.md");
        std::fs::write(&path, "fresh disk bytes").expect("write temp file");
        let (event_tx, _event_rx) = bounded(CHANNEL_CAPACITY);
        let (reply_tx, reply_rx) = bounded(CHANNEL_CAPACITY);
        let mut watched = HashMap::new();
        let mut watched_dirs = HashSet::new();
        let buffer_id = continuity_buffer::BufferId::new();

        let should_shutdown = handle_request(
            FileIoRequest::RecheckFile {
                buffer_id,
                path: path.clone(),
                reply: Some(reply_tx),
            },
            &event_tx,
            None,
            &mut watched,
            &mut watched_dirs,
        );

        assert!(!should_shutdown);
        let event = reply_rx.try_recv().expect("receive recheck reply");
        let FileIoEvent::Rechecked {
            buffer_id: got_id,
            content,
            file,
        } = event
        else {
            panic!("expected rechecked event");
        };
        assert_eq!(got_id, buffer_id);
        assert_eq!(content, "fresh disk bytes");
        assert_eq!(file.path, path);
        // The recheck arms the external-change watch so a restored buffer
        // that was never watched this session begins observing edits.
        assert!(watched.contains_key(&normalize_path(&path)));
    }

    #[test]
    fn save_refuses_when_disk_changed_since_expected() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("note.md");
        std::fs::write(&path, "external content").expect("seed file");
        let (event_tx, event_rx) = bounded(CHANNEL_CAPACITY);
        let mut watched = HashMap::new();
        let mut watched_dirs = HashSet::new();
        let buffer_id = continuity_buffer::BufferId::new();

        // expected_hash is deliberately stale (0) — it can't match the
        // current on-disk hash, so the save must be refused.
        handle_request(
            FileIoRequest::SaveBuffer {
                buffer_id,
                path: path.clone(),
                content: "my unsaved edits".into(),
                expected_hash: Some(0),
            },
            &event_tx,
            None,
            &mut watched,
            &mut watched_dirs,
        );

        match event_rx.try_recv().expect("conflict event") {
            FileIoEvent::SaveConflict {
                buffer_id: got,
                content,
                file,
                ..
            } => {
                assert_eq!(got, buffer_id);
                // The current disk content is surfaced for the diff/reload.
                assert_eq!(content, "external content");
                assert_eq!(file.path, path);
            }
            other => panic!("expected SaveConflict, got {other:?}"),
        }
        // The write was refused: the external content is intact on disk.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "external content"
        );
    }

    #[test]
    fn save_writes_when_expected_hash_matches_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("note.md");
        std::fs::write(&path, "in sync").expect("seed file");
        let current = read_file(&path).expect("read");
        let (event_tx, event_rx) = bounded(CHANNEL_CAPACITY);
        let mut watched = HashMap::new();
        let mut watched_dirs = HashSet::new();

        handle_request(
            FileIoRequest::SaveBuffer {
                buffer_id: continuity_buffer::BufferId::new(),
                path: path.clone(),
                content: "updated".into(),
                expected_hash: Some(current.file.hash),
            },
            &event_tx,
            None,
            &mut watched,
            &mut watched_dirs,
        );

        assert!(matches!(
            event_rx.try_recv().expect("saved event"),
            FileIoEvent::Saved { .. }
        ));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "updated");
    }

    #[test]
    fn save_with_no_expected_hash_force_writes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("note.md");
        std::fs::write(&path, "external content").expect("seed file");
        let (event_tx, event_rx) = bounded(CHANNEL_CAPACITY);
        let mut watched = HashMap::new();
        let mut watched_dirs = HashSet::new();

        // expected_hash None = "keep mine" / save-as → unconditional write.
        handle_request(
            FileIoRequest::SaveBuffer {
                buffer_id: continuity_buffer::BufferId::new(),
                path: path.clone(),
                content: "forced over external".into(),
                expected_hash: None,
            },
            &event_tx,
            None,
            &mut watched,
            &mut watched_dirs,
        );

        assert!(matches!(
            event_rx.try_recv().expect("saved event"),
            FileIoEvent::Saved { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "forced over external"
        );
    }

    #[test]
    fn recheck_missing_file_is_silent() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("gone.md");
        let (event_tx, event_rx) = bounded(CHANNEL_CAPACITY);
        let (reply_tx, reply_rx) = bounded(CHANNEL_CAPACITY);
        let mut watched = HashMap::new();
        let mut watched_dirs = HashSet::new();

        let should_shutdown = handle_request(
            FileIoRequest::RecheckFile {
                buffer_id: continuity_buffer::BufferId::new(),
                path,
                reply: Some(reply_tx),
            },
            &event_tx,
            None,
            &mut watched,
            &mut watched_dirs,
        );

        assert!(!should_shutdown);
        // A missing file produces neither a reply nor a failure banner —
        // the rope stays canonical and a later save recreates the path.
        assert!(reply_rx.try_recv().is_err());
        assert!(event_rx.try_recv().is_err());
    }
}
