//! Typed byte / line spaces for the display map.
//!
//! `SourceByte` and `DisplayByte` are both `u32` underneath but the compiler
//! refuses to mix them. The cost is `#[repr(transparent)]` (zero) and a small
//! conversion API; the benefit is that the bug class "I forgot
//! `+ margins.left`" (or any similar source ↔ display offset confusion)
//! becomes unrepresentable.

use std::fmt;
use std::ops::Range;

macro_rules! u32_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[repr(transparent)]
        #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Default)]
        pub struct $name(pub u32);

        impl $name {
            /// Zero value.
            pub const ZERO: Self = Self(0);

            /// Construct from `usize`. Panics in debug if the value exceeds
            /// `u32::MAX`; in release the conversion truncates. Callers should
            /// only build these from rope-sized indices, which fit comfortably
            /// in `u32` (Continuity caps individual buffers far below 4 GiB).
            #[must_use]
            pub fn from_usize(n: usize) -> Self {
                debug_assert!(
                    n <= u32::MAX as usize,
                    concat!(stringify!($name), "::from_usize overflow")
                );
                Self(n as u32)
            }

            /// Convert back to `usize`.
            #[must_use]
            pub fn as_usize(self) -> usize {
                self.0 as usize
            }

            /// Get the raw `u32` value.
            #[must_use]
            pub fn raw(self) -> u32 {
                self.0
            }

            /// Saturating add.
            #[must_use]
            pub fn saturating_add(self, rhs: u32) -> Self {
                Self(self.0.saturating_add(rhs))
            }

            /// Saturating sub.
            #[must_use]
            pub fn saturating_sub(self, rhs: u32) -> Self {
                Self(self.0.saturating_sub(rhs))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

u32_newtype!(
    SourceByte,
    "UTF-8 byte offset into the *source* rope. Canonical position space for selections, undo, persistence, search."
);
u32_newtype!(
    DisplayByte,
    "UTF-8 byte offset into a *display* line. Layout-local. Never persists; never crosses a rope boundary."
);
u32_newtype!(
    DisplayUtf16,
    "UTF-16 code-unit offset into a *display* line. DirectWrite speaks UTF-16; this is the unit handed to `IDWriteTextLayout` APIs."
);
u32_newtype!(SourceLine, "0-based logical line index in the source rope.");
u32_newtype!(
    DisplayLine,
    "0-based visible-line index after reveal/replace/wrap/fold. Different lines for one source line when soft-wrap fires; one (or zero) display line for many source lines when a fold collapses them."
);

/// Pair of source and display byte offsets — useful for round-trip tests and
/// hit-test results.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct ByteMapping {
    /// Source byte offset.
    pub source: SourceByte,
    /// Display byte offset.
    pub display: DisplayByte,
}

/// Convenience: turn a `Range<usize>` into a `Range<SourceByte>`.
#[must_use]
pub fn source_range(r: Range<usize>) -> Range<SourceByte> {
    SourceByte::from_usize(r.start)..SourceByte::from_usize(r.end)
}

/// Convenience: turn a `Range<usize>` into a `Range<DisplayByte>`.
#[must_use]
pub fn display_range(r: Range<usize>) -> Range<DisplayByte> {
    DisplayByte::from_usize(r.start)..DisplayByte::from_usize(r.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtypes_have_zero_runtime_overhead() {
        assert_eq!(std::mem::size_of::<SourceByte>(), 4);
        assert_eq!(std::mem::size_of::<DisplayByte>(), 4);
        assert_eq!(std::mem::size_of::<DisplayUtf16>(), 4);
        assert_eq!(std::mem::size_of::<SourceLine>(), 4);
        assert_eq!(std::mem::size_of::<DisplayLine>(), 4);
    }

    #[test]
    fn roundtrip_usize_conversion() {
        assert_eq!(SourceByte::from_usize(42).as_usize(), 42);
        assert_eq!(DisplayLine::from_usize(0).raw(), 0);
        assert_eq!(SourceByte::ZERO.as_usize(), 0);
    }

    #[test]
    fn saturating_arithmetic() {
        let a = SourceByte::from_usize(5);
        assert_eq!(a.saturating_add(3).as_usize(), 8);
        assert_eq!(a.saturating_sub(10).as_usize(), 0);
        assert_eq!(SourceByte(u32::MAX).saturating_add(1).raw(), u32::MAX);
    }

    #[test]
    fn debug_and_display_formats_are_legible() {
        assert_eq!(format!("{:?}", SourceByte(7)), "SourceByte(7)");
        assert_eq!(format!("{}", SourceByte(7)), "7");
        assert_eq!(format!("{:?}", DisplayLine(0)), "DisplayLine(0)");
    }

    #[test]
    fn ordering_is_by_inner_value() {
        assert!(SourceByte(3) < SourceByte(4));
        assert!(DisplayByte(10) > DisplayByte(2));
    }

    #[test]
    fn range_helpers_build_typed_ranges() {
        let s = source_range(2..7);
        assert_eq!(s.start.raw(), 2);
        assert_eq!(s.end.raw(), 7);
        let d = display_range(0..3);
        assert_eq!(d.start.raw(), 0);
        assert_eq!(d.end.raw(), 3);
    }
}
