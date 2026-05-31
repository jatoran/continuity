//! Source positions: `(line, byte_in_line)`.
//!
//! Positions are 0-indexed source coordinates. We store byte offsets within
//! the line rather than absolute byte offsets in the rope so that an external
//! file merge that adds lines above doesn't invalidate every cached position.

use ropey::Rope;
use serde::{Deserialize, Serialize};

use crate::Error;

/// A source position: 0-indexed line plus a 0-indexed byte offset into that
/// line.
///
/// `byte_in_line` always points at a UTF-8 boundary (callers are responsible).
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Position {
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed byte offset within `line`.
    pub byte_in_line: u32,
}

impl Position {
    /// The position at the very start of any buffer.
    pub const ZERO: Self = Self {
        line: 0,
        byte_in_line: 0,
    };

    /// Construct a `Position`.
    #[must_use]
    pub const fn new(line: u32, byte_in_line: u32) -> Self {
        Self { line, byte_in_line }
    }

    /// Translate to the absolute source byte offset in `rope`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::OutOfBounds`] when the line or byte fall outside the
    /// rope's content, or when `byte_in_line` is not a UTF-8 char boundary.
    pub fn to_byte_offset(self, rope: &Rope) -> Result<usize, Error> {
        let line = self.line as usize;
        if line > rope.len_lines() {
            return Err(Error::OutOfBounds(line));
        }
        let line_start = rope
            .try_line_to_byte(line)
            .map_err(|_| Error::OutOfBounds(line))?;
        let byte = line_start + self.byte_in_line as usize;
        if byte > rope.len_bytes() {
            return Err(Error::OutOfBounds(byte));
        }
        if !is_rope_char_boundary(rope, byte) {
            return Err(Error::OutOfBounds(byte));
        }
        Ok(byte)
    }

    /// Construct a `Position` from an absolute byte offset in `rope`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::OutOfBounds`] when `byte` exceeds the rope length or
    /// does not land on a UTF-8 char boundary.
    pub fn from_byte_offset(rope: &Rope, byte: usize) -> Result<Self, Error> {
        if byte > rope.len_bytes() {
            return Err(Error::OutOfBounds(byte));
        }
        if !is_rope_char_boundary(rope, byte) {
            return Err(Error::OutOfBounds(byte));
        }
        let line = rope.byte_to_line(byte);
        let line_start = rope.line_to_byte(line);
        Ok(Self {
            line: u32::try_from(line).map_err(|_| Error::OutOfBounds(line))?,
            byte_in_line: u32::try_from(byte - line_start)
                .map_err(|_| Error::OutOfBounds(byte - line_start))?,
        })
    }
}

fn is_rope_char_boundary(rope: &Rope, byte: usize) -> bool {
    if byte > rope.len_bytes() {
        return false;
    }
    if byte == rope.len_bytes() {
        return true;
    }
    let Some((chunk, chunk_byte_start, _, _)) = rope.get_chunk_at_byte(byte) else {
        return false;
    };
    chunk.is_char_boundary(byte.saturating_sub(chunk_byte_start))
}

#[cfg(test)]
mod tests {
    use ropey::Rope;

    use super::*;

    #[test]
    fn zero_translates_to_byte_zero() {
        let rope = Rope::from_str("hello\nworld");
        assert_eq!(Position::ZERO.to_byte_offset(&rope).unwrap(), 0);
    }

    #[test]
    fn line_two_resolves_correctly() {
        let rope = Rope::from_str("hello\nworld");
        assert_eq!(Position::new(1, 0).to_byte_offset(&rope).unwrap(), 6);
        assert_eq!(Position::new(1, 5).to_byte_offset(&rope).unwrap(), 11);
    }

    #[test]
    fn round_trip_through_byte_offset() {
        let rope = Rope::from_str("a\nbb\nccc\ndddd");
        for byte in 0..=rope.len_bytes() {
            let p = Position::from_byte_offset(&rope, byte).unwrap();
            assert_eq!(p.to_byte_offset(&rope).unwrap(), byte, "byte {byte}");
        }
    }

    #[test]
    fn out_of_bounds_byte_fails() {
        let rope = Rope::from_str("hi");
        assert!(Position::from_byte_offset(&rope, 999).is_err());
    }

    #[test]
    fn out_of_bounds_position_fails() {
        let rope = Rope::from_str("hi");
        assert!(Position::new(99, 0).to_byte_offset(&rope).is_err());
    }

    #[test]
    fn interior_utf8_byte_position_fails() {
        let rope = Rope::from_str("é");
        assert!(Position::new(0, 1).to_byte_offset(&rope).is_err());
    }

    #[test]
    fn interior_utf8_byte_offset_fails() {
        let rope = Rope::from_str("é");
        assert!(Position::from_byte_offset(&rope, 1).is_err());
    }
}
