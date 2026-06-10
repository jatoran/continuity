#![warn(missing_docs)]
//! The singleton editor state machine.
//!
//! `core` is the only crate that owns mutable buffer state. It receives
//! `EditorMessage`s on a channel and broadcasts `EditEvent`s back. All
//! buffer mutation must flow through this crate.

pub mod clock;
pub(crate) mod dispatch;
pub(crate) mod edit_indent_shift;
pub mod edit_indent_subtree;
pub(crate) mod edit_inline;
pub(crate) mod edit_line_text;
pub(crate) mod edit_line_text_helpers;
pub(crate) mod edit_lines;
pub(crate) mod edit_lines_movement;
pub(crate) mod edit_list;
pub(crate) mod edit_markdown;
pub(crate) mod edit_markdown_blocks;
pub(crate) mod edit_markdown_strip;
pub(crate) mod edit_normalize;
pub(crate) mod edit_pairs;
pub(crate) mod edit_planning;
pub(crate) mod edit_words;
pub mod error;
pub mod handle;
pub mod indent_fold_provider;
pub mod markdown_heading_fold_provider;
pub mod message;
pub mod policy;
pub mod rope_edit_delta_points;
pub mod selection_coalesce;
pub mod selection_edit;
pub mod state;
pub(crate) mod trace;
pub mod undo;
pub mod wpm;

pub use clock::{Clock, SystemClock};
pub use edit_indent_subtree::{
    all_top_level_subtrees, indent_subtree, line_indent, next_sibling_subtree,
    previous_sibling_subtree, IndentRange,
};
pub use edit_pairs::AutoPairConfig;
pub use error::Error;
pub use handle::EditorHandle;
pub use indent_fold_provider::{compute_indent_fold_byte_ranges, IndentFoldByteRange};
pub use markdown_heading_fold_provider::compute_heading_fold_byte_ranges;
pub use message::{BufferSummary, CoreMemoryStats, EditEvent, EditorMessage, EditorSnapshot};
pub use policy::{edit_byte_delta, SnapshotPolicy, SnapshotTracker, SnapshotTrigger};
pub use rope_edit_delta_points::{EditPoint, RopeEditDeltaWithPoints};
pub use selection_edit::{
    CaseKind, EmphasisKind, IndentUnit, LineEnding, SelectionEdit, SelectionEditPlan, SortKind,
};
pub use state::EditorState;
pub use undo::{CoalesceKind, UndoOrchestrator, COALESCE_WINDOW_MS};
pub use wpm::WpmTracker;
