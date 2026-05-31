#![warn(missing_docs)]
//! Text primitives: positions, ranges, selections, and edit operations
//! over a `ropey::Rope`.
//!
//! Foundation layer. No internal workspace dependencies.

pub mod bidi;
pub mod edit;
pub mod error;
pub mod position;
pub mod range;
pub mod rope_edit_delta;
pub mod select;
pub mod selection;

pub use bidi::{level_at, paragraph_direction, ParagraphDirection};
pub use edit::EditOp;
pub use error::Error;
pub use position::Position;
pub use range::Range;
pub use rope_edit_delta::{
    transform_byte_through, transform_container_range_through,
    transform_container_range_through_chain, transform_range_through,
    transform_range_through_chain, RopeEditDelta,
};
pub use selection::{Selection, SelectionKind};
