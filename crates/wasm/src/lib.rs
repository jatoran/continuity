#![warn(missing_docs)]
//! Thin WebAssembly binding for Continuity's synchronous portable engine.
//!
//! Raw `wasm-bindgen` names are internal to the npm package. Stable JavaScript
//! and TypeScript consumers use the facade under `packages/editor`.

mod editor;
mod history;
mod projection;
mod projection_cache;
mod report;

pub use editor::WasmEditor;
pub use history::{HistoryState, UndoGroupState};
pub use projection::{compute_presentation_report, compute_projection_report};
pub use report::{ChangeReport, EditorSnapshotReport, PositionReport, SelectionReport};
