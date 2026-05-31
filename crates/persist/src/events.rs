//! δ.3 — typed events emitted by the persistence thread for UI banner
//! display.
//!
//! Today the persist thread emits a structured event on **every**
//! fire-and-forget write failure (the cases where the previous
//! behavior was a `stderr eprintln!` only). The events are consumed
//! by the registry, which fans them out to every live window via
//! [`crate::WindowControl::PersistEvent`]; each window converts them
//! into a sticky `FileBanner` so the durability promise is visible.
//!
//! Thread ownership: the [`crate::PersistHandle`] owns the
//! `Receiver<PersistEvent>` that downstream consumers borrow via
//! [`crate::PersistHandle::events`]. The send side lives on the
//! persistence thread; one `Sender` is dropped when the loop exits.

/// One typed event emitted by the persistence thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistEvent {
    /// A write-side persist operation failed (SQLite error, zstd
    /// error, disk-full, permission-denied, …). The string is the
    /// `thiserror`-formatted error message; the kind names the
    /// `PersistMessage` variant that produced it so the UI banner can
    /// distinguish "edit-log append failed" from "snapshot write
    /// failed" without having to embed the underlying SQLite error
    /// taxonomy.
    WriteFailed {
        /// Stable short name of the failing operation.
        kind: PersistOperation,
        /// Human-readable error message from the underlying error.
        message: String,
    },
    /// The persistence thread is exiting normally (after a
    /// `Shutdown` message). Emitted as the loop's last event before
    /// the `Sender` drops so consumers can distinguish a clean
    /// shutdown from a thread panic.
    ThreadStopped,
}

/// Names the originating `PersistMessage` for a `WriteFailed` event.
/// Kept as an enum (rather than `&'static str`) so a future router
/// can dispatch banners by kind without string-matching.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PersistOperation {
    /// `PersistMessage::AppendEdit` — the marquee durability path.
    AppendEdit,
    /// `PersistMessage::SaveSnapshot` (fire-and-forget variant).
    SaveSnapshot,
    /// `PersistMessage::UpsertBuffer`.
    UpsertBuffer,
    /// `PersistMessage::TouchBuffer`.
    TouchBuffer,
    /// `PersistMessage::PruneCoveredEdits`.
    PruneCoveredEdits,
    /// `PersistMessage::MoveToTrash`.
    MoveToTrash,
    /// `PersistMessage::Backup` (fire-and-forget variant).
    Backup,
    /// `PersistMessage::SetSynchronous`.
    SetSynchronous,
    /// `PersistMessage::WriteUndoGroup`.
    WriteUndoGroup,
    /// `PersistMessage::SaveWindow` (fire-and-forget variant).
    SaveWindow,
    /// `PersistMessage::SetBufferFile` (fire-and-forget variant).
    SetBufferFile,
    /// `PersistMessage::SetSnapshotLabel` (fire-and-forget variant).
    SetSnapshotLabel,
    /// `PersistMessage::RecordMetricsDelta`.
    RecordMetricsDelta,
}

impl PersistOperation {
    /// Stable short name for log lines / banner text.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppendEdit => "append_edit",
            Self::SaveSnapshot => "save_snapshot",
            Self::UpsertBuffer => "upsert_buffer",
            Self::TouchBuffer => "touch_buffer",
            Self::PruneCoveredEdits => "prune_covered_edits",
            Self::MoveToTrash => "move_to_trash",
            Self::Backup => "backup",
            Self::SetSynchronous => "set_synchronous",
            Self::WriteUndoGroup => "write_undo_group",
            Self::SaveWindow => "save_window",
            Self::SetBufferFile => "set_buffer_file",
            Self::SetSnapshotLabel => "set_snapshot_label",
            Self::RecordMetricsDelta => "record_metrics_delta",
        }
    }
}

impl PersistEvent {
    /// Build the user-visible banner text for this event. Kept on
    /// the type so every consumer renders identical wording.
    #[must_use]
    pub fn banner_text(&self) -> String {
        match self {
            Self::WriteFailed { kind, message } => format!(
                "Persistence write failed ({}). Some recent edits may not be durable — restart to recover. Details: {message}",
                kind.as_str()
            ),
            Self::ThreadStopped => {
                "Persistence thread stopped. Further edits will not be durable — restart continuity to recover.".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_failed_banner_names_kind_and_message() {
        let ev = PersistEvent::WriteFailed {
            kind: PersistOperation::AppendEdit,
            message: "disk full".into(),
        };
        let text = ev.banner_text();
        assert!(text.contains("append_edit"));
        assert!(text.contains("disk full"));
    }

    #[test]
    fn thread_stopped_banner_mentions_restart() {
        let text = PersistEvent::ThreadStopped.banner_text();
        assert!(text.to_lowercase().contains("restart"));
    }
}
