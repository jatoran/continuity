// ε.5 ships the worker foundation only; until the integration slice
// wires `Window::on_paint` to dispatch + validate worker results,
// these types read "never used".
#![allow(dead_code)]
//! [`ProjectionStamp`] — every input that changes the projection's
//! pixels — and the [`StampMismatchField`] diff trace that names the
//! first input to drift between request and live paint.

use std::ops::Range;

use continuity_display_map::{FoldRange, FoldSignature, ImageRowReservation};
use continuity_layout::FontStateId;

/// Stamp identifying every input that changes the projection's pixels.
///
/// Two projections built with the same stamp produce the same
/// [`crate::projection_worker::ProjectionResult::frame_display`]. The UI
/// thread carries one of these per submitted request and re-stamps the
/// live paint inputs when it polls; only a result whose stamp matches
/// the current paint stamp is safe to use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectionStamp {
    /// Owning buffer (`BufferId.as_uuid().as_u128()`).
    pub document: u128,
    /// Rope revision the projection was/will be built against.
    pub rope_revision: u64,
    /// Decoration revision (after any `transformed_through` shift the
    /// UI thread applied), or `None` for the undecorated path.
    pub decoration_revision: Option<u64>,
    /// Worker parse revision captured before stale decorations are
    /// re-labelled by `transformed_through`.
    pub decoration_parse_revision: Option<u64>,
    /// `caret_bytes` hash — caret position participates in the
    /// projection through markdown-marker reveal.
    pub caret_signature: u64,
    /// Fold set hash — every `FoldRange` start/end participates.
    pub fold_signature: u64,
    /// Image-row reservation hash. Empty reservations hash to a
    /// deterministic seed so the bare path still stamps cleanly.
    pub image_reservations_signature: u64,
    /// Soft-wrap width in DIPs. `0` disables wrap.
    pub wrap_width_dip: u32,
    /// Font configuration stamp.
    pub font_state: FontStateId,
    /// Absolute display-row range the realization covers.
    pub viewport_rows: Range<u32>,
    /// Overscan rows above and below `viewport_rows`.
    pub overscan: u32,
}

/// FNV-1a 64-bit prime and seed. Matches the hand-rolled hasher
/// pattern already used by `display_prewarm_cache::fold_signature`.
const FNV1A_SEED: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn fnv1a_mix(h: u64, x: u64) -> u64 {
    (h ^ x).wrapping_mul(FNV1A_PRIME)
}

/// Which field of [`ProjectionStamp`] differs when a worker result is
/// rejected for stamp mismatch. Named so the worker-miss trace can
/// identify which input drifted between the request and the live paint
/// — useful for deciding whether stale-result acceptance would have
/// been safe for the case in question, without changing the (still
/// exact-stamp) acceptance contract.
///
/// ε.5d investigation outcome: a stale-acceptance contract that would
/// have helped sustained typing requires inspecting rope deltas and
/// decoration diffs against the visible viewport (see roadmap_v4 ε.5
/// status). This enum gives the empirical signal — "which field
/// actually drifts" — without committing to that contract.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum StampMismatchField {
    /// Different `BufferId` (cross-buffer worker result; should not
    /// happen with per-window workers but listed for completeness).
    Document,
    /// `rope_revision` drifted — the typing-burst case.
    RopeRevision,
    /// `decoration_revision` drifted — decoration worker caught up
    /// since dispatch (or fell behind).
    DecorationRevision,
    /// Underlying decoration parse content changed while the
    /// transformed decoration revision label stayed the same.
    DecorationParseRevision,
    /// Caret moved.
    CaretSignature,
    /// Fold set changed.
    FoldSignature,
    /// Image reservations changed.
    ImageReservationsSignature,
    /// Soft-wrap width changed.
    WrapWidth,
    /// Font configuration changed.
    FontState,
    /// Viewport row range changed (scroll, resize, or font reflow).
    Viewport,
    /// Overscan amount changed.
    Overscan,
}

impl StampMismatchField {
    /// Stable trace spelling used by `WorkerMissReason::as_str`.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::RopeRevision => "rope_revision",
            Self::DecorationRevision => "decoration_revision",
            Self::DecorationParseRevision => "decoration_parse_revision",
            Self::CaretSignature => "caret_signature",
            Self::FoldSignature => "fold_signature",
            Self::ImageReservationsSignature => "image_reservations_signature",
            Self::WrapWidth => "wrap_width",
            Self::FontState => "font_state",
            Self::Viewport => "viewport",
            Self::Overscan => "overscan",
        }
    }
}

impl ProjectionStamp {
    /// First field that differs between `self` and `other`, or `None`
    /// when the stamps are equal. Comparison order is fixed (most
    /// likely to drift first) so the trace is stable for diffing
    /// across runs: rope → decoration → parse → caret → viewport →
    /// fold → image_reservations → wrap_width → font_state →
    /// overscan → document.
    #[must_use]
    pub(crate) fn diff_field(&self, other: &Self) -> Option<StampMismatchField> {
        if self.rope_revision != other.rope_revision {
            return Some(StampMismatchField::RopeRevision);
        }
        if self.decoration_revision != other.decoration_revision {
            return Some(StampMismatchField::DecorationRevision);
        }
        if self.decoration_parse_revision != other.decoration_parse_revision {
            return Some(StampMismatchField::DecorationParseRevision);
        }
        if self.caret_signature != other.caret_signature {
            return Some(StampMismatchField::CaretSignature);
        }
        if self.viewport_rows != other.viewport_rows {
            return Some(StampMismatchField::Viewport);
        }
        if self.fold_signature != other.fold_signature {
            return Some(StampMismatchField::FoldSignature);
        }
        if self.image_reservations_signature != other.image_reservations_signature {
            return Some(StampMismatchField::ImageReservationsSignature);
        }
        if self.wrap_width_dip != other.wrap_width_dip {
            return Some(StampMismatchField::WrapWidth);
        }
        if self.font_state != other.font_state {
            return Some(StampMismatchField::FontState);
        }
        if self.overscan != other.overscan {
            return Some(StampMismatchField::Overscan);
        }
        if self.document != other.document {
            return Some(StampMismatchField::Document);
        }
        None
    }

    /// Hash a caret-byte vector into a stable signature.
    #[must_use]
    pub(crate) fn caret_signature(caret_bytes: &[usize]) -> u64 {
        let mut h = FNV1A_SEED;
        h = fnv1a_mix(h, caret_bytes.len() as u64);
        for byte in caret_bytes {
            h = fnv1a_mix(h, *byte as u64);
        }
        h
    }

    /// Hash a fold-range slice into a stable signature.
    #[must_use]
    pub(crate) fn fold_signature(folds: &[FoldRange]) -> u64 {
        FoldSignature::compute(folds)
    }

    /// Hash an image-reservation slice into a stable signature.
    #[must_use]
    pub(crate) fn image_reservations_signature(reservations: &[ImageRowReservation]) -> u64 {
        let mut h = FNV1A_SEED;
        h = fnv1a_mix(h, reservations.len() as u64);
        for reservation in reservations {
            h = fnv1a_mix(h, u64::from(reservation.source_line.raw()));
            h = fnv1a_mix(h, u64::from(reservation.reserved_display_rows));
        }
        h
    }
}
