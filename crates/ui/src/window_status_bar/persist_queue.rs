//! α.1 persist-queue chip — visible in the right-hand chip lane while
//! the persist thread has uncommitted bytes; collapses the moment the
//! queue drains. Chip text is the static label
//! [`PERSIST_QUEUE_CHIP_LABEL`]: a byte count would tick on every drain
//! and `status_motion` would emit a 180 ms transient per tick, flooding
//! the bar with strobing fades. A static label fades in once on
//! appearance and out once on drain. The byte count surfaces only in
//! the hover tooltip via [`format_persist_queue_depth`].
//!
//! Thread ownership: UI thread of one window.

use continuity_render::{StatusBarSegmentDraw, StatusBarSegmentKind};

use crate::Window;

/// Static visible label of the α.1 persist-queue chip.
pub(crate) const PERSIST_QUEUE_CHIP_LABEL: &str = "↑ syncing";

/// Format the unflushed-bytes count as a compact hover-tooltip label.
#[must_use]
pub(crate) fn format_persist_queue_depth(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * 1024;
    if bytes >= MIB {
        format!("↑ {:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("↑ {:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("↑ {bytes} B")
    }
}

impl Window {
    /// α.1 — build the persist-queue-depth chip when the persist client
    /// is wired and its backlog is non-zero. Returns `None` otherwise.
    pub(super) fn persist_queue_chip(&self) -> Option<StatusBarSegmentDraw> {
        let bytes = self.persist_client.as_ref()?.unflushed_bytes();
        if bytes == 0 {
            return None;
        }
        Some(StatusBarSegmentDraw {
            text: PERSIST_QUEUE_CHIP_LABEL.to_string(),
            kind: StatusBarSegmentKind::PersistQueueChip,
            hover: Some(format_persist_queue_depth(bytes)),
            alpha: 1.0,
        })
    }
}
