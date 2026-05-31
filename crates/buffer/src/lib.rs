#![warn(missing_docs)]
//! Buffer state: a revisioned `Rope`, undo tree, decoration cache, and
//! per-pane view state.
//!
//! All mutation of a [`Buffer`] is the responsibility of the `core` crate's
//! editor thread (single-writer rule). Other threads receive [`RopeSnapshot`]s.

pub mod buffer;
pub mod checksum;
pub mod error;
pub mod id;
pub mod inverse_op;
pub mod metadata;
pub mod revision;
pub(crate) mod selection_clamp;
pub mod snapshot;
pub mod undo;

pub use buffer::{derive_title, Buffer};
pub use checksum::{
    full_walk_rope, update_for_edit as update_running_checksum, CHECKSUM_VERIFY_INTERVAL,
    FNV_OFFSET_BASIS,
};
pub use error::Error;
pub use id::{BufferId, UndoGroupId, WindowId};
pub use inverse_op::compute_inverse_op;
pub use metadata::FileAssociation;
pub use revision::Revision;
pub use snapshot::{RopeSnapshot, RopeSnapshotRegistry};
pub use undo::{EditRecord, UndoGroup, UndoTree};
