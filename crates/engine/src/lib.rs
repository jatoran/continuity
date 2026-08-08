#![warn(missing_docs)]
//! Synchronous, storage-neutral Continuity editor engine.
//!
//! The caller selects the thread that owns [`EngineState`]. This crate never
//! creates threads, windows, databases, filesystem state, or host callbacks.

mod change;
mod delta_history;
mod edit_indent_shift;
pub mod edit_indent_subtree;
mod edit_inline;
mod edit_line_text;
mod edit_line_text_helpers;
mod edit_lines;
mod edit_lines_movement;
mod edit_list;
mod edit_markdown;
mod edit_markdown_blocks;
mod edit_markdown_strip;
mod edit_normalize;
pub mod edit_pairs;
mod edit_planning;
mod edit_words;
mod engine;
pub mod error;
pub mod reconcile_diff;
mod replay;
pub mod rope_edit_delta_points;
pub mod selection_coalesce;
pub mod selection_edit;
pub mod state;
mod undo;

pub use change::{AppliedChange, ChangeBatch, EngineEvent, MutationKind, UndoGroupMeta};
pub use delta_history::DeltaHistory;
pub use edit_indent_subtree::{
    all_top_level_subtrees, indent_subtree, line_indent, next_sibling_subtree,
    previous_sibling_subtree, IndentRange,
};
pub use edit_pairs::AutoPairConfig;
pub use engine::{Engine, EngineSnapshot, IdGenerator, SystemIdGenerator};
pub use error::Error;
pub use replay::{replay_change_batches, ReplayedState};
pub use rope_edit_delta_points::{EditPoint, RopeEditDeltaWithPoints};
pub use selection_edit::{
    CaseKind, EmphasisKind, IndentUnit, LineEnding, SelectionEdit, SelectionEditPlan, SortKind,
};
pub use state::EngineState;
pub use undo::{CoalesceKind, COALESCE_WINDOW_MS};
