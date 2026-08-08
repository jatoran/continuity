//! Portable projection identity and stale-result mismatch classification.

use std::ops::Range;

use crate::{FoldRange, FoldSignature, ImageRowReservation};

/// Stamp identifying every input that changes projection pixels.
///
/// The font identity is generic so a backend can use its own compact font
/// cache key without adding a platform layout dependency to `display_map`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionStamp<FontState> {
    /// Host document identity.
    pub document: u128,
    /// Rope revision.
    pub rope_revision: u64,
    /// Decoration revision, or `None` for undecorated projection.
    pub decoration_revision: Option<u64>,
    /// Underlying parse revision before forward transformation.
    pub decoration_parse_revision: Option<u64>,
    /// Hash of caret byte offsets that control marker reveal.
    pub caret_signature: u64,
    /// Hash of folded source ranges.
    pub fold_signature: u64,
    /// Hash of inline-image row reservations.
    pub image_reservations_signature: u64,
    /// Soft-wrap width in device-independent pixels; zero disables wrap.
    pub wrap_width_dip: u32,
    /// Backend-defined font/cache identity.
    pub font_state: FontState,
    /// Absolute display rows covered by the requested realization.
    pub viewport_rows: Range<u32>,
    /// Rows realized above and below the viewport.
    pub overscan: u32,
}

/// First projection input that differs between a built result and live state.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StampMismatchField {
    /// Document identity.
    Document,
    /// Rope revision.
    RopeRevision,
    /// Decoration revision.
    DecorationRevision,
    /// Underlying decoration parse revision.
    DecorationParseRevision,
    /// Caret reveal signature.
    CaretSignature,
    /// Fold signature.
    FoldSignature,
    /// Image reservation signature.
    ImageReservationsSignature,
    /// Soft-wrap width.
    WrapWidth,
    /// Backend font state.
    FontState,
    /// Viewport rows.
    Viewport,
    /// Overscan rows.
    Overscan,
}

impl StampMismatchField {
    /// Stable trace spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
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

impl<FontState: Eq> ProjectionStamp<FontState> {
    /// Return the first differing field in stable diagnostic priority order.
    #[must_use]
    pub fn diff_field(&self, other: &Self) -> Option<StampMismatchField> {
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
}

const FNV1A_SEED: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn fnv1a_mix(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(FNV1A_PRIME)
}

impl<FontState> ProjectionStamp<FontState> {
    /// Hash caret byte offsets into a stable reveal signature.
    #[must_use]
    pub fn caret_signature(caret_bytes: &[usize]) -> u64 {
        let mut hash = fnv1a_mix(FNV1A_SEED, caret_bytes.len() as u64);
        for byte in caret_bytes {
            hash = fnv1a_mix(hash, *byte as u64);
        }
        hash
    }

    /// Hash folded ranges into the display-map fold signature.
    #[must_use]
    pub fn fold_signature(folds: &[FoldRange]) -> u64 {
        FoldSignature::compute(folds)
    }

    /// Hash image reservations into a stable geometry signature.
    #[must_use]
    pub fn image_reservations_signature(reservations: &[ImageRowReservation]) -> u64 {
        let mut hash = fnv1a_mix(FNV1A_SEED, reservations.len() as u64);
        for reservation in reservations {
            hash = fnv1a_mix(hash, u64::from(reservation.source_line.raw()));
            hash = fnv1a_mix(hash, u64::from(reservation.reserved_display_rows));
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> ProjectionStamp<u64> {
        ProjectionStamp {
            document: 1,
            rope_revision: 2,
            decoration_revision: Some(2),
            decoration_parse_revision: Some(2),
            caret_signature: ProjectionStamp::<u64>::caret_signature(&[0, 4]),
            fold_signature: ProjectionStamp::<u64>::fold_signature(&[]),
            image_reservations_signature: ProjectionStamp::<u64>::image_reservations_signature(&[]),
            wrap_width_dip: 80,
            font_state: 9,
            viewport_rows: 0..40,
            overscan: 10,
        }
    }

    #[test]
    fn identical_stamps_have_no_mismatch() {
        let stamp = stamp();
        assert_eq!(stamp.diff_field(&stamp), None);
    }

    #[test]
    fn mismatch_priority_is_stable() {
        let left = stamp();
        let mut right = stamp();
        right.document = 99;
        right.rope_revision = 3;
        assert_eq!(
            left.diff_field(&right),
            Some(StampMismatchField::RopeRevision)
        );
        assert_eq!(StampMismatchField::RopeRevision.as_str(), "rope_revision");
    }
}
