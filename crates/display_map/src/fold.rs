//! Fold ranges — collapsed regions that the renderer treats as invisible.
//!
//! A fold is a half-open source byte range whose every byte becomes
//! `DisplaySegment::Hidden`. Multi-line folds may collapse several source
//! lines into one (or zero) display lines; the builder is responsible
//! for emitting at most one display line per non-empty wrap of the
//! remaining visible bytes on each affected source line.

use std::ops::Range;

use crate::id::SourceByte;

/// Stable fold-set signature used in row-index and projection stamps.
pub struct FoldSignature;

impl FoldSignature {
    /// Canonical signature when the effective fold set is empty.
    pub const EMPTY: u64 = 0;

    /// Compute a stable signature for the active fold ranges.
    ///
    /// Empty and empty-after-normalization fold sets always return
    /// [`Self::EMPTY`], so "no folded lines" cannot drift between
    /// caches just because a caller used a different hasher seed.
    #[must_use]
    pub fn compute(folds: &[FoldRange]) -> u64 {
        let mut has_effective_fold = false;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for fold in folds {
            if fold.start.raw() >= fold.end.raw() {
                continue;
            }
            has_effective_fold = true;
            h ^= u64::from(fold.start.raw());
            h = h.wrapping_mul(0x100_0000_01b3);
            h ^= u64::from(fold.end.raw());
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        if has_effective_fold {
            h
        } else {
            Self::EMPTY
        }
    }
}

/// A folded source byte range. `start..end` is half-open and never empty.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FoldRange {
    /// Inclusive start byte.
    pub start: SourceByte,
    /// Exclusive end byte.
    pub end: SourceByte,
}

impl FoldRange {
    /// Construct from a byte range. Returns `None` if `start >= end`.
    #[must_use]
    pub fn new(start: SourceByte, end: SourceByte) -> Option<Self> {
        if start.raw() < end.raw() {
            Some(Self { start, end })
        } else {
            None
        }
    }

    /// `true` if `byte` is strictly inside the fold.
    #[must_use]
    pub fn contains(&self, byte: SourceByte) -> bool {
        byte.raw() >= self.start.raw() && byte.raw() < self.end.raw()
    }

    /// The fold as a `Range<SourceByte>`.
    #[must_use]
    pub fn as_range(&self) -> Range<SourceByte> {
        self.start..self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_range() {
        assert!(FoldRange::new(SourceByte(5), SourceByte(5)).is_none());
        assert!(FoldRange::new(SourceByte(6), SourceByte(5)).is_none());
    }

    #[test]
    fn contains_is_half_open() {
        let f = FoldRange::new(SourceByte(2), SourceByte(5)).unwrap();
        assert!(!f.contains(SourceByte(1)));
        assert!(f.contains(SourceByte(2)));
        assert!(f.contains(SourceByte(4)));
        assert!(!f.contains(SourceByte(5)));
    }
}
