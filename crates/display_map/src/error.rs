//! Error type for the display-map crate.

use thiserror::Error;

/// Errors raised by [`crate::DisplayMapBuilder`].
#[derive(Debug, Error)]
pub enum Error {
    /// A caret byte was past the end of the rope at the snapshot's revision.
    #[error("caret byte {byte} out of bounds (rope length {len})")]
    CaretOutOfBounds {
        /// The offending caret byte.
        byte: usize,
        /// The rope length at snapshot time.
        len: usize,
    },

    /// A fold range extended past the end of the rope.
    #[error("fold range {start}..{end} out of bounds (rope length {len})")]
    FoldOutOfBounds {
        /// Start of the offending fold.
        start: usize,
        /// End of the offending fold.
        end: usize,
        /// The rope length at snapshot time.
        len: usize,
    },

    /// The width-measure callback returned a non-finite or negative value.
    #[error("WidthMeasure returned a non-finite or negative value ({0})")]
    BadMeasurement(f32),
}
