//! Half-open ranges of [`Position`]s: `[start, end)`.

use serde::{Deserialize, Serialize};

use crate::Position;

/// A half-open range of positions.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    /// Inclusive start.
    pub start: Position,
    /// Exclusive end.
    pub end: Position,
}

impl Range {
    /// Construct a range. Caller is responsible for `start <= end`.
    #[must_use]
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// An empty range at `at`.
    #[must_use]
    pub const fn empty(at: Position) -> Self {
        Self { start: at, end: at }
    }

    /// `true` when start equals end.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Return a range with `start <= end`.
    #[must_use]
    pub fn ordered(self) -> Self {
        if self.start <= self.end {
            self
        } else {
            Self {
                start: self.end,
                end: self.start,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_range_is_empty() {
        let r = Range::empty(Position::new(2, 3));
        assert!(r.is_empty());
        assert_eq!(r.start, r.end);
    }

    #[test]
    fn nonempty_range() {
        let r = Range::new(Position::ZERO, Position::new(0, 5));
        assert!(!r.is_empty());
    }
}
