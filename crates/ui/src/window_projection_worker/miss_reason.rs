//! Classification of why a per-paint worker result was rejected.
//!
//! The variants are the alphabet of the `projection_worker_miss`
//! trace event; renaming them is a wire-format change.

use crate::projection_worker::StampMismatchField;

/// Why a worker result was rejected for this paint.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkerMissReason {
    /// Worker has not been spawned yet (no `text_format` at the time of
    /// the first paint, or `Window` was constructed in a test that never
    /// painted).
    WorkerAbsent,
    /// Worker has not produced a result since the cell was last drained.
    NotReady,
    /// Worker result exists but its stamp does not match the current
    /// paint inputs (the typical typing-burst case: the worker was
    /// building for the previous keystroke). The carried
    /// [`StampMismatchField`] names *which* input drifted; production
    /// still rejects on any field mismatch (exact-stamp acceptance) so
    /// the field is purely diagnostic.
    StampMismatch(StampMismatchField),
    /// First paint cannot use a worker result because the worker has
    /// never produced one yet — separated from `NotReady` so the
    /// trace tells the two apart.
    FirstPaint,
}

impl WorkerMissReason {
    /// Stable trace spelling. Stamp-mismatch variants name the
    /// differing field (`stamp_mismatch_rope_revision`, …) so the
    /// trace identifies the dominant drift cause without needing to
    /// inspect raw stamps.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WorkerAbsent => "worker_absent",
            Self::NotReady => "not_ready",
            Self::StampMismatch(field) => match field {
                StampMismatchField::Document => "stamp_mismatch_document",
                StampMismatchField::RopeRevision => "stamp_mismatch_rope_revision",
                StampMismatchField::DecorationRevision => "stamp_mismatch_decoration_revision",
                StampMismatchField::DecorationParseRevision => {
                    "stamp_mismatch_decoration_parse_revision"
                }
                StampMismatchField::CaretSignature => "stamp_mismatch_caret_signature",
                StampMismatchField::FoldSignature => "stamp_mismatch_fold_signature",
                StampMismatchField::ImageReservationsSignature => {
                    "stamp_mismatch_image_reservations_signature"
                }
                StampMismatchField::WrapWidth => "stamp_mismatch_wrap_width",
                StampMismatchField::FontState => "stamp_mismatch_font_state",
                StampMismatchField::Viewport => "stamp_mismatch_viewport",
                StampMismatchField::Overscan => "stamp_mismatch_overscan",
            },
            Self::FirstPaint => "first_paint",
        }
    }
}
