//! File-I/O worker used by Phase 15.
//!
//! Disk reads, writes, and directory watches live on this worker thread.
//! UI threads enqueue requests and poll completion events.

#[cfg(test)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use continuity_buffer::{BufferId, FileAssociation};
use crossbeam_channel::{bounded, Receiver, Sender};

use crate::file_io_primitives::read_file;
#[cfg(test)]
use crate::file_io_primitives::{is_self_write, normalize_path, write_file};
#[cfg(test)]
use crate::file_io_worker::{handle_notify, WatchedFile};
use crate::pane_tree::PaneId;
use crate::DirectoryEntry;

use crate::file_io_worker::CHANNEL_CAPACITY;

/// Owner of the file-I/O worker thread.
pub struct FileIoService {
    tx: Sender<FileIoRequest>,
    events: Receiver<FileIoEvent>,
    join: Option<JoinHandle<()>>,
}

/// Clone-able client used by windows.
#[derive(Clone)]
pub struct FileIoClient {
    tx: Sender<FileIoRequest>,
    events: Receiver<FileIoEvent>,
}

/// A synchronously read file ready to install as a file-associated
/// buffer.
#[derive(Clone, Debug)]
pub struct StartupOpenedFile {
    /// Decoded text content.
    pub content: String,
    /// Filesystem association metadata captured from the read.
    pub file: FileAssociation,
    /// Encoding notice emitted when the bytes were not clean UTF-8.
    pub encoding_notice: Option<&'static str>,
}

/// Result event emitted by the file-I/O worker.
#[derive(Clone, Debug)]
pub enum FileIoEvent {
    /// A file was read and decoded.
    Opened {
        /// Pane that requested the open, if known.
        target_pane: Option<PaneId>,
        /// Decoded text content.
        content: String,
        /// File metadata.
        file: FileAssociation,
    },
    /// One directory under an opened folder root was listed.
    DirectoryListed {
        /// Canonical opened root.
        root: PathBuf,
        /// Relative directory path from `root`.
        relative: PathBuf,
        /// Bounded child entries.
        entries: Vec<DirectoryEntry>,
        /// True when entries were capped.
        truncated: bool,
    },
    /// A buffer was saved to disk.
    Saved {
        /// Saved buffer.
        buffer_id: BufferId,
        /// File metadata after write.
        file: FileAssociation,
    },
    /// A save was **refused** because the file changed on disk since the
    /// buffer last synced (its raw-byte hash no longer matches the
    /// expected fingerprint). The write did not happen — overwriting would
    /// have silently destroyed the external edit. Carries the current disk
    /// bytes so the UI can roll back the optimistic clean state and raise
    /// the reload / keep-mine / show-diff conflict banner. Closes the race
    /// where a save beats the asynchronous `notify` watcher.
    SaveConflict {
        /// Buffer whose save was refused.
        buffer_id: BufferId,
        /// File path.
        path: PathBuf,
        /// Current on-disk content.
        content: String,
        /// Current filesystem association (mtime + raw/content hashes).
        file: FileAssociation,
    },
    /// A watched file was reloaded for an existing buffer.
    Reloaded {
        /// Target buffer.
        buffer_id: BufferId,
        /// Decoded text content.
        content: String,
        /// File metadata after read.
        file: FileAssociation,
    },
    /// A one-shot disk recheck completed (session restore or explicit
    /// refresh). Carries the current disk bytes + fingerprint so the
    /// window can reconcile a possibly-stale buffer. Unlike
    /// [`FileIoEvent::ExternalChanged`], the worker does *not* gate this
    /// on a self-write comparison — the window owns the clean/dirty
    /// decision via [`crate::window_file_reconcile`].
    Rechecked {
        /// Target buffer.
        buffer_id: BufferId,
        /// Current disk content.
        content: String,
        /// File metadata after read.
        file: FileAssociation,
    },
    /// A watched file changed outside the editor.
    ExternalChanged {
        /// Associated buffer.
        buffer_id: BufferId,
        /// File path.
        path: PathBuf,
        /// Current disk content.
        content: String,
        /// File metadata after read.
        file: FileAssociation,
    },
    /// δ.3 — a watched file was deleted or renamed away externally.
    /// The rope/buffer is kept in memory (the rope is canonical; the
    /// file is just an export) — a follow-up `file.save` recreates
    /// the path. The UI banners this so the user knows the disk side
    /// is gone.
    Deleted {
        /// Associated buffer (still in memory).
        buffer_id: BufferId,
        /// The path that disappeared.
        path: PathBuf,
    },
    /// δ.3 — a watched file was opened with non-UTF-8 / unexpected
    /// encoding heuristics; the content was opened with U+FFFD
    /// replacement characters. Emitted in addition to `Opened` /
    /// `Reloaded`, never instead of them. The UI banners this so the
    /// user knows they shouldn't blindly re-save (re-export would
    /// commit the replacement characters to disk).
    EncodingNotice {
        /// Path the encoding heuristic fired on.
        path: PathBuf,
        /// Short label for the detected encoding (e.g. `"UTF-16 LE"`,
        /// `"non-UTF-8"`).
        encoding: &'static str,
    },
    /// A request failed.
    Failed {
        /// Buffer the failed request targeted, if any. Set for `save` /
        /// `reload` so the UI can roll back an optimistic state change.
        buffer_id: Option<BufferId>,
        /// Human-readable operation name.
        operation: &'static str,
        /// Path involved, if any.
        path: Option<PathBuf>,
        /// Error message.
        reason: String,
    },
}

/// Read a startup path synchronously before window threads spawn.
///
/// This reuses the same decode and fingerprint contract as the file-I/O
/// worker, but avoids routing startup `Open with` files through the
/// shared worker event receiver where a restored multi-window session
/// could race to consume the event from the wrong window.
pub fn read_startup_file(path: &std::path::Path) -> std::io::Result<StartupOpenedFile> {
    let result = read_file(path)?;
    Ok(StartupOpenedFile {
        content: result.content,
        file: result.file,
        encoding_notice: result.encoding_notice,
    })
}

pub(crate) enum FileIoRequest {
    OpenFiles {
        paths: Vec<PathBuf>,
        target_pane: Option<PaneId>,
        reply: Option<Sender<FileIoEvent>>,
    },
    ListDirectory {
        root: PathBuf,
        relative: PathBuf,
    },
    SaveBuffer {
        buffer_id: BufferId,
        path: PathBuf,
        content: String,
        /// Last on-disk raw-byte hash this buffer synced to. When `Some`,
        /// the worker re-reads the file before writing and refuses the
        /// save (emitting `SaveConflict`) if the current hash differs — an
        /// external change happened since. `None` forces an
        /// unconditional write (save-as / explicit "keep mine").
        expected_hash: Option<u64>,
    },
    ReloadBuffer {
        buffer_id: BufferId,
        path: PathBuf,
    },
    RecheckFile {
        buffer_id: BufferId,
        path: PathBuf,
        reply: Option<Sender<FileIoEvent>>,
    },
    WatchFile {
        buffer_id: BufferId,
        file: FileAssociation,
    },
    Shutdown,
}

impl FileIoService {
    /// Spawn the file-I/O worker.
    pub fn spawn() -> Self {
        let (tx, rx) = bounded::<FileIoRequest>(CHANNEL_CAPACITY);
        let (event_tx, events) = bounded::<FileIoEvent>(CHANNEL_CAPACITY);
        let join = thread::Builder::new()
            .name("continuity-file-io".into())
            .spawn(move || crate::file_io_worker::worker_loop(rx, event_tx))
            .expect("spawn continuity-file-io thread");
        Self {
            tx,
            events,
            join: Some(join),
        }
    }

    /// Clone-able client for windows.
    #[must_use]
    pub fn client(&self) -> FileIoClient {
        FileIoClient {
            tx: self.tx.clone(),
            events: self.events.clone(),
        }
    }

    /// Shut the worker down and join it.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(FileIoRequest::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for FileIoService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl FileIoClient {
    /// Request file imports.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub fn open_files(&self, paths: Vec<PathBuf>, target_pane: Option<PaneId>) -> bool {
        self.tx
            .send(FileIoRequest::OpenFiles {
                paths,
                target_pane,
                reply: None,
            })
            .is_ok()
    }

    /// Request file imports with completions routed to one window.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub(crate) fn open_files_with_reply(
        &self,
        paths: Vec<PathBuf>,
        target_pane: Option<PaneId>,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::OpenFiles {
                paths,
                target_pane,
                reply: Some(reply),
            })
            .is_ok()
    }

    /// Request a bounded listing for one directory under an opened root.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub(crate) fn list_directory(&self, root: PathBuf, relative: PathBuf) -> bool {
        self.tx
            .send(FileIoRequest::ListDirectory { root, relative })
            .is_ok()
    }

    /// Request a save.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub fn save_buffer(
        &self,
        buffer_id: BufferId,
        path: PathBuf,
        content: String,
        expected_hash: Option<u64>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::SaveBuffer {
                buffer_id,
                path,
                content,
                expected_hash,
            })
            .is_ok()
    }

    /// Begin or refresh a file watch.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub(crate) fn watch_file(&self, buffer_id: BufferId, file: FileAssociation) -> bool {
        self.tx
            .send(FileIoRequest::WatchFile { buffer_id, file })
            .is_ok()
    }

    /// Reload an associated file for an existing buffer.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub(crate) fn reload_buffer(&self, buffer_id: BufferId, path: PathBuf) -> bool {
        self.tx
            .send(FileIoRequest::ReloadBuffer { buffer_id, path })
            .is_ok()
    }

    /// Read a file once and report its current bytes/fingerprint back to
    /// one window so it can reconcile a possibly-stale buffer (session
    /// restore, explicit refresh). Also (re)arms the external-change watch
    /// for the path. Completions route to `reply` so only the requesting
    /// window reconciles.
    ///
    /// # Errors
    ///
    /// Returns `false` when the worker has exited.
    pub(crate) fn recheck_file(
        &self,
        buffer_id: BufferId,
        path: PathBuf,
        reply: Sender<FileIoEvent>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::RecheckFile {
                buffer_id,
                path,
                reply: Some(reply),
            })
            .is_ok()
    }

    /// Borrow the worker event receiver.
    #[must_use]
    pub fn events(&self) -> &Receiver<FileIoEvent> {
        &self.events
    }
}

// File-I/O primitives (read_file, write_file, decode_file_bytes,
// install_watch, send_failed, normalize_path, is_self_write,
// system_time_ms, fnv1a_64) and the `ReadFileResult` carrier live in
// the sibling `file_io_primitives.rs` to keep this module under the
// 600-line cap. See `crate::file_io_primitives`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        let file = write_file(&path, "hello").unwrap();
        let result = read_file(&path).unwrap();
        assert_eq!(result.content, "hello");
        assert_eq!(file.hash, result.file.hash);
        assert!(result.encoding_notice.is_none());
    }

    /// δ.3 — UTF-8 BOM is stripped silently and is NOT reported as an
    /// encoding notice (it's still UTF-8).
    #[test]
    fn read_file_strips_utf8_bom_silently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.md");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"hello");
        std::fs::write(&path, &bytes).unwrap();
        let result = read_file(&path).unwrap();
        assert_eq!(result.content, "hello");
        assert!(result.encoding_notice.is_none());
    }

    /// δ.3 — UTF-16 LE BOM triggers UTF-16 decode + an encoding notice.
    #[test]
    fn read_file_decodes_utf16_le_bom_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf16.md");
        let mut bytes = vec![0xFF, 0xFE]; // UTF-16 LE BOM
        for ch in "hi".encode_utf16() {
            bytes.extend_from_slice(&ch.to_le_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        let result = read_file(&path).unwrap();
        assert_eq!(result.content, "hi");
        assert_eq!(result.encoding_notice, Some("UTF-16 LE"));
    }

    /// δ.3 — UTF-16 BE BOM triggers UTF-16 decode + an encoding notice.
    #[test]
    fn read_file_decodes_utf16_be_bom_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf16be.md");
        let mut bytes = vec![0xFE, 0xFF]; // UTF-16 BE BOM
        for ch in "hi".encode_utf16() {
            bytes.extend_from_slice(&ch.to_be_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        let result = read_file(&path).unwrap();
        assert_eq!(result.content, "hi");
        assert_eq!(result.encoding_notice, Some("UTF-16 BE"));
    }

    /// δ.3 — invalid UTF-8 bytes still produce a `String` (via lossy
    /// decode) but flag the encoding notice so the UI can banner.
    #[test]
    fn read_file_reports_non_utf8_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.txt");
        // 0xE9 alone is invalid as UTF-8 (start of a 3-byte sequence
        // with no continuation bytes). It would be `é` in Latin-1.
        std::fs::write(&path, [b'h', b'i', 0xE9]).unwrap();
        let result = read_file(&path).unwrap();
        assert_eq!(result.encoding_notice, Some("non-UTF-8"));
        // U+FFFD replacement byte sequence ends the string.
        assert!(result.content.starts_with("hi"));
        assert!(result.content.contains('\u{FFFD}'));
    }

    /// δ.3 — when a watched file disappears between the notify
    /// event firing and our follow-up read, `handle_notify` must
    /// emit `FileIoEvent::Deleted` and prune the `watched` entry.
    #[test]
    fn handle_notify_emits_deleted_when_path_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.md");
        std::fs::write(&path, "hi").unwrap();

        let buffer_id = BufferId::new();
        let file = FileAssociation::new(path.clone(), 0, 0);
        let mut watched: HashMap<PathBuf, WatchedFile> = HashMap::new();
        watched.insert(
            normalize_path(&path),
            WatchedFile {
                buffer_id,
                file: file.clone(),
            },
        );

        // Delete the file so the follow-up read in handle_notify fails.
        std::fs::remove_file(&path).unwrap();

        let (tx, rx) = bounded::<FileIoEvent>(8);
        let event: notify::Event =
            notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
                .add_path(path.clone());
        handle_notify(Ok(event), &tx, &mut watched);

        let received = rx.try_recv().expect("expected a Deleted event");
        match received {
            FileIoEvent::Deleted {
                buffer_id: got_id,
                path: got_path,
            } => {
                assert_eq!(got_id, buffer_id);
                assert_eq!(got_path, path);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        // Entry pruned so subsequent events for the path don't re-fire.
        assert!(!watched.contains_key(&normalize_path(&path)));
    }

    #[test]
    fn self_write_suppression_matches_post_save_fingerprint() {
        // A notify event whose on-disk read matches the expected post-save
        // fingerprint must be classified as a self-write — i.e. the same
        // bytes the editor just wrote.
        let expected = FileAssociation::new(PathBuf::from("note.md"), 1_700_000_000_000, 42);
        let same = expected.clone();
        assert!(is_self_write(&same, &expected));
    }

    #[test]
    fn self_write_suppression_rejects_real_external_edit() {
        let expected = FileAssociation::new(PathBuf::from("note.md"), 1_700_000_000_000, 42);
        // Different hash → real external edit.
        let observed_hash = FileAssociation::new(expected.path.clone(), expected.mtime_ms, 999);
        assert!(!is_self_write(&observed_hash, &expected));
        // Same hash but a later mtime → still real (e.g., touch).
        let observed_mtime = FileAssociation::new(expected.path.clone(), 1_700_000_001_000, 42);
        assert!(!is_self_write(&observed_mtime, &expected));
    }
}
