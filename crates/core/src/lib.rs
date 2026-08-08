#![warn(missing_docs)]
//! Native threaded host for the synchronous editor engine.
//!
//! The Windows core actor owns one [`continuity_engine::Engine`], receives
//! [`EditorMessage`] values, adapts change batches to SQLite, and broadcasts
//! [`EditEvent`] values. Direct embedders own an engine without this actor.

pub mod clock;
pub(crate) mod dispatch;
pub mod error;
pub mod handle;
pub mod indent_fold_provider;
pub mod markdown_heading_fold_provider;
pub mod message;
mod persistence_bridge;
pub mod policy;
pub(crate) mod trace;

pub use clock::{Clock, SystemClock};
pub use continuity_engine::{
    all_top_level_subtrees, indent_subtree, line_indent, next_sibling_subtree,
    previous_sibling_subtree, AutoPairConfig, IndentRange,
};
pub use continuity_engine::{edit_indent_subtree, edit_pairs, selection_edit};
pub use continuity_engine::{rope_edit_delta_points, selection_coalesce, state};
pub use continuity_engine::{
    CaseKind, EmphasisKind, IndentUnit, LineEnding, SelectionEdit, SelectionEditPlan, SortKind,
};
pub use continuity_engine::{CoalesceKind, COALESCE_WINDOW_MS};
pub use continuity_engine::{EditPoint, EngineState, RopeEditDeltaWithPoints};
/// Compatibility name for the engine-owned buffer collection.
pub type EditorState = EngineState;
pub use error::Error;
pub use handle::EditorHandle;
pub use indent_fold_provider::{compute_indent_fold_byte_ranges, IndentFoldByteRange};
pub use markdown_heading_fold_provider::compute_heading_fold_byte_ranges;
pub use message::{BufferSummary, CoreMemoryStats, EditEvent, EditorMessage, EditorSnapshot};
pub use policy::{edit_byte_delta, SnapshotPolicy, SnapshotTracker, SnapshotTrigger};
