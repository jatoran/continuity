//! [`EditorHandle`] buffer lifecycle and inspection methods.
//!
//! Opening, adopting, snapshotting, enumerating, and updating the file
//! association of buffers. Each method is a thin wrapper that ships an
//! [`EditorMessage`] to the core thread via [`EditorHandle::round_trip`]
//! and blocks on the reply.

use continuity_buffer::{Buffer, BufferId, FileAssociation};

use crate::handle::EditorHandle;
use crate::message::{BufferSummary, EditorMessage, EditorSnapshot};
use crate::Error;

impl EditorHandle {
    /// Request a fresh buffer with the given initial content.
    pub fn open_buffer(&self, content: impl Into<String>) -> BufferId {
        self.round_trip(|reply| EditorMessage::OpenBuffer {
            content: content.into(),
            reply,
        })
    }

    /// Request a fresh file-associated buffer with the given initial
    /// content.
    pub fn open_file_buffer(&self, content: impl Into<String>, file: FileAssociation) -> BufferId {
        self.round_trip(|reply| EditorMessage::OpenFileBuffer {
            content: content.into(),
            file,
            reply,
        })
    }

    /// Adopt a buffer recovered from disk.
    pub fn adopt_buffer(
        &self,
        buffer: Buffer,
        next_seq: u64,
        last_snapshot_at_ms: i64,
    ) -> BufferId {
        self.round_trip(|reply| EditorMessage::AdoptBuffer {
            buffer,
            next_seq,
            last_snapshot_at_ms,
            reply,
        })
    }

    /// Snapshot a buffer.
    pub fn snapshot(&self, buffer_id: BufferId) -> Option<EditorSnapshot> {
        self.round_trip(|reply| EditorMessage::Snapshot { buffer_id, reply })
    }

    /// Update a buffer's file association.
    ///
    /// # Errors
    ///
    /// Returns whatever the core thread reports.
    pub fn set_file_association(
        &self,
        buffer_id: BufferId,
        file: Option<FileAssociation>,
    ) -> Result<(), Error> {
        self.round_trip(|reply| EditorMessage::SetFileAssociation {
            buffer_id,
            file,
            reply,
        })
    }

    /// Enumerate every open buffer.
    ///
    /// Returned summaries are produced on the core thread and contain the
    /// derived title, first non-empty line, revision, and line count for
    /// each buffer. Order is unspecified.
    pub fn list_buffers(&self) -> Vec<BufferSummary> {
        self.round_trip(|reply| EditorMessage::ListBuffers { reply })
    }
}
