//! The per-buffer revision counter.
//!
//! Every accepted edit advances the revision. Decoration results, channel
//! messages, and persisted edit log rows all carry the revision they were
//! computed against — receivers that find the buffer has advanced past their
//! revision discard their stale work.

/// A monotonically increasing revision number for a buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Revision(pub u64);

impl Revision {
    /// The revision of a freshly-created buffer.
    pub const INITIAL: Self = Self(0);

    /// The next revision after `self`.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Borrow the inner counter for serialization.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_increments_by_one() {
        assert_eq!(Revision::INITIAL.next(), Revision(1));
        assert_eq!(Revision(7).next(), Revision(8));
    }

    #[test]
    fn ordering_is_numeric() {
        assert!(Revision(0) < Revision(1));
        assert!(Revision(99) > Revision(98));
    }
}
