//! File-I/O worker requests, results, service, and client.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use continuity_buffer::{BufferId, FileAssociation};
use continuity_config::{VaultConfig, VaultWorkspaceState};
use crossbeam_channel::{bounded, Receiver, Sender};

use crate::file_io_primitives::read_file;
use crate::pane_tree::PaneId;
use crate::window_config::FileOpenDisposition;
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
    pub(crate) tx: Sender<FileIoRequest>,
    events: Receiver<FileIoEvent>,
}

/// A synchronously read file ready to install as a file-associated buffer.
#[derive(Clone, Debug)]
pub struct StartupOpenedFile {
    /// Decoded text content.
    pub content: String,
    /// Filesystem association metadata captured from the read.
    pub file: FileAssociation,
    /// Encoding notice emitted when the bytes were not clean UTF-8.
    pub encoding_notice: Option<&'static str>,
}

/// Kind of vault entry to create.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultEntryKind {
    /// Empty Markdown file.
    File,
    /// Folder.
    Directory,
}

/// Result event emitted by the file-I/O worker.
#[derive(Clone, Debug)]
pub enum FileIoEvent {
    /// A requested folder was canonicalized and inspected for a vault marker.
    FolderInspected {
        /// Canonical folder the user selected.
        requested_root: PathBuf,
        /// Nearest vault root, or the selected folder for a normal browse.
        root: PathBuf,
        /// Validated vault config when a marker was found.
        config: Option<VaultConfig>,
        /// Portable vault UI state, absent for ordinary folder browsing.
        workspace: Option<VaultWorkspaceState>,
    },
    /// An opened-tree mutation completed and affected these relative paths.
    VaultEntriesChanged {
        /// Canonical opened-folder root.
        root: PathBuf,
        /// Parent directory to refresh.
        refresh_relative: PathBuf,
        /// Old and new relative prefixes for a move.
        moved: Option<(PathBuf, PathBuf)>,
        /// Deleted relative prefix, when recycling.
        deleted: Option<PathBuf>,
    },
    /// A watched vault marker was reloaded and revalidated.
    VaultConfigChanged {
        /// Vault root owning the marker.
        root: PathBuf,
        /// New validated configuration.
        config: VaultConfig,
    },
    /// A file was read and decoded.
    Opened {
        /// Pane that requested the open, if known.
        target_pane: Option<PaneId>,
        /// Decoded text content.
        content: String,
        /// File metadata.
        file: FileAssociation,
        /// Requested placement semantics.
        disposition: FileOpenDisposition,
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
    InspectFolder {
        path: PathBuf,
        reply: Sender<FileIoEvent>,
    },
    InitializeVault {
        path: PathBuf,
        reply: Sender<FileIoEvent>,
    },
    CreateVaultEntry {
        root: PathBuf,
        parent: PathBuf,
        kind: VaultEntryKind,
        reply: Sender<FileIoEvent>,
    },
    MoveVaultEntry {
        root: PathBuf,
        source: PathBuf,
        destination_directory: PathBuf,
        reply: Sender<FileIoEvent>,
    },
    RenameTreeEntry {
        root: PathBuf,
        source: PathBuf,
        new_name: String,
        reply: Sender<FileIoEvent>,
    },
    DeleteVaultEntry {
        root: PathBuf,
        relative: PathBuf,
        reply: Sender<FileIoEvent>,
    },
    PersistVaultWorkspace {
        root: PathBuf,
        state: VaultWorkspaceState,
        reply: Sender<FileIoEvent>,
    },
    OpenFiles {
        paths: Vec<PathBuf>,
        target_pane: Option<PaneId>,
        reply: Option<Sender<FileIoEvent>>,
        /// Window to wake after routed completions are enqueued.
        wake_window: Option<usize>,
        disposition: FileOpenDisposition,
    },
    ListDirectory {
        root: PathBuf,
        relative: PathBuf,
        config: Option<VaultConfig>,
        reply: Sender<FileIoEvent>,
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
        /// Where `Saved` / `SaveConflict` / `Failed` for this save are sent.
        /// Routed to the requesting window so a sibling window draining the
        /// shared event channel cannot steal the completion (which would
        /// leave the owning window's autosave wedged `in_flight`). `None`
        /// falls back to the shared channel for tests.
        reply: Option<Sender<FileIoEvent>>,
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
                wake_window: None,
                disposition: FileOpenDisposition::NewWindow,
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
        wake_window: usize,
        disposition: FileOpenDisposition,
    ) -> bool {
        self.tx
            .send(FileIoRequest::OpenFiles {
                paths,
                target_pane,
                reply: Some(reply),
                wake_window: Some(wake_window),
                disposition,
            })
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
        reply: Option<Sender<FileIoEvent>>,
    ) -> bool {
        self.tx
            .send(FileIoRequest::SaveBuffer {
                buffer_id,
                path,
                content,
                expected_hash,
                reply,
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
#[path = "file_io/tests.rs"]
mod tests;
