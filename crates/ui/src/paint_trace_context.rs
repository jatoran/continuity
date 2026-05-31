//! Thread-local context the tracer stamps onto every emitted line.
//!
//! Pulled out of [`crate::paint_trace`] to keep that file under the
//! 600-line conventions cap. Two orthogonal contexts live here:
//!
//! - **edit_seq** — monotonic UI-side edit counter, bound at each
//!   edit-funnel entry, mirrored to the core thread via the
//!   `EditorMessage` payload and to the persist worker via
//!   `PersistMessage::AppendEdit`.
//! - **invalidate_reason** — captured at `invalidate_with_reason` and
//!   consumed by `PaintTrace::new` for `paint:TOTAL reason=…`.
//!
//! Both contexts are read-and-stamp at line emit time; nothing crosses
//! threads on its own. Cross-thread propagation is the message
//! payload's job.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process counter for UI-side edits. Bumped at every
/// edit-funnel entry. Persists in `CURRENT_EDIT_SEQ` for the duration
/// of the funnel so every `EventScope` / `log_event` call emitted
/// underneath can stamp `edit_seq=N` into its detail.
static EDIT_SEQ: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Per-thread current edit sequence. Set by [`bind_edit_seq`] at
    /// funnel entry, cleared on guard drop. Cross-thread propagation
    /// goes through the `EditorMessage::ApplySelectionEdit { edit_seq }`
    /// / `PersistMessage::AppendEdit { edit_seq }` fields.
    static CURRENT_EDIT_SEQ: Cell<Option<u64>> = const { Cell::new(None) };

    /// Highest-priority reason among invalidates pending for the next
    /// paint. Set by `note_invalidate_request_with_reason`,
    /// read-and-cleared by `PaintTrace::new` to stamp `reason=` on
    /// `paint:TOTAL`. Valid reasons include `invalidate_rect`,
    /// `scroll_anim`, `decoration_delivered`, `prewarm_tick`,
    /// `caret_blink`, `theme_apply`, `external_invalidate`, and
    /// `dpi_changed`.
    pub(crate) static INVALIDATE_REASON: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// Allocate the next edit seq. Wraps once per `u64::MAX` events.
#[inline]
pub(crate) fn next_edit_seq() -> u64 {
    EDIT_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Currently-bound edit seq, if any.
#[inline]
pub(crate) fn current_edit_seq() -> Option<u64> {
    CURRENT_EDIT_SEQ.with(|c| c.get())
}

/// Bind `seq` as the thread-local edit seq until the returned guard
/// drops.
#[must_use = "the guard restores the previous edit_seq on drop"]
pub(crate) fn bind_edit_seq(seq: u64) -> EditSeqGuard {
    let previous = CURRENT_EDIT_SEQ.with(|c| c.replace(Some(seq)));
    EditSeqGuard { previous }
}

/// RAII guard restoring the previous edit seq on drop.
pub(crate) struct EditSeqGuard {
    previous: Option<u64>,
}

impl Drop for EditSeqGuard {
    fn drop(&mut self) {
        let prev = self.previous;
        CURRENT_EDIT_SEQ.with(|c| c.set(prev));
    }
}

/// Take-and-clear the pending invalidate reason. Called by
/// `PaintTrace::new`.
#[inline]
pub(crate) fn take_invalidate_reason() -> Option<&'static str> {
    INVALIDATE_REASON.with(|c| c.replace(None))
}

/// Peek at the pending paint reason without consuming it. Used by
/// `WM_PAINT` wndproc detail before `PaintTrace::new` takes it.
#[inline]
pub(crate) fn peek_invalidate_reason() -> Option<&'static str> {
    INVALIDATE_REASON.with(|c| c.get())
}

/// Merge a new invalidate reason into the pending paint reason.
/// Highest priority wins when Win32 coalesces multiple invalidates into
/// one `WM_PAINT`.
#[inline]
pub(crate) fn merge_invalidate_reason(reason: &'static str) {
    INVALIDATE_REASON.with(|c| {
        if c.get().is_none_or(|existing| {
            invalidate_reason_priority(reason) > invalidate_reason_priority(existing)
        }) {
            c.set(Some(reason));
        }
    });
}

fn invalidate_reason_priority(reason: &str) -> u8 {
    match reason {
        "dpi_changed" => 90,
        "theme_apply" => 80,
        "external_invalidate" => 70,
        "projection_delivered" => 65,
        "decoration_delivered" => 60,
        "prewarm_tick" => 50,
        "scroll_anim" => 40,
        "image_animation" => 35,
        "motion_tick" => 30,
        "caret_blink" => 20,
        "invalidate_rect" => 10,
        _ => 0,
    }
}

/// Format `detail` with `edit_seq=N ` prepended when one is bound.
/// Returns `detail` unchanged when no seq is bound (zero allocation
/// in the common no-edit-context case).
pub(crate) fn stamp_with_edit_seq(detail: &str) -> std::borrow::Cow<'_, str> {
    match current_edit_seq() {
        Some(seq) if !detail.is_empty() => {
            std::borrow::Cow::Owned(format!("edit_seq={seq} {detail}"))
        }
        Some(seq) => std::borrow::Cow::Owned(format!("edit_seq={seq}")),
        None => std::borrow::Cow::Borrowed(detail),
    }
}
