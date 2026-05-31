#![warn(missing_docs)]
//! Tree-sitter incremental parsing and markdown decoration computation.
//!
//! Decoration is a pure function of `(RopeSnapshot, Revision) -> Decorations`:
//! no mutation, no globals, no I/O. Stale results (revision mismatch) are
//! discarded by the consumer.
//!
//! Phase 10 grows this crate beyond Phase-1 block-span extraction:
//!
//! - [`InlineSpan`] / [`InlineKind`] / [`MarkerKind`] — inline data model
//!   (emphasis, code, links, image refs, checkboxes, structural markers).
//! - [`Decorations`] — the per-revision aggregate that flows from worker
//!   threads to the UI.
//! - [`DecoratePool`] / [`DecorateRequest`] / [`DecorateResult`] — the
//!   worker pool. Workers consume snapshots, produce `Decorations`.
//! - [`DecorationCache`] — per-buffer revision-keyed UI-side cache; same
//!   staleness rule as the Phase 9 layout cache.
//! - [`Language`] / [`detect`] — per-buffer language identification used
//!   by the `language` context atom.

pub mod autolink;
pub mod cache;
pub mod decorations;
pub mod decorations_diff;
pub mod decorations_incremental;
mod decorations_memory;
pub mod decorations_transform;
pub mod error;
pub mod focus_span;
pub mod footnotes;
pub mod heading_task_progress;
pub mod headings;
pub mod image_link;
pub mod inline;
pub mod inline_color;
pub mod inline_text;
pub mod language;
pub mod parser;
pub mod pool;
pub mod rainbow;
mod request_queue;
pub mod sections;
pub mod spans;
pub mod syntax;
pub mod table_block_fixup;
pub mod table_eval;
pub mod table_formula;
mod table_formula_parser;
pub mod tables;
pub mod toc;
pub mod tree_cache;
pub mod tree_sitter_alloc;
mod worker_watchdog;

pub use autolink::{auto_links, AutoLink, AutoLinkKind};
pub use cache::{DecorationCache, DecorationCacheCounters};
pub use decorations::Decorations;
pub use decorations_incremental::{EditPoint, RopeEditDeltaWithPoints};
pub use error::Error;
pub use focus_span::{line_span, paragraph_span, sentence_span, FocusSpan};
pub use footnotes::footnote_definition_spans;
pub use heading_task_progress::{task_progress_per_heading, TaskProgress};
pub use headings::{headings, HeadingEntry};
pub use inline::{block_inline_spans, ByteRange, InlineKind, InlineSpan, MarkerKind};
pub use inline_color::{inline_color_spans, parse_hex_rgba, InlineColorKind, InlineColorSpan};
pub use language::{detect, Language};
pub use parser::MarkdownParser;
pub use pool::parse_trace::{DecorationFullParseReason, DecorationParseTrace};
pub use pool::tree_cache_registry::TreeCacheRegistry;
pub use pool::{
    empty_deltas, DecoratePool, DecorateRequest, DecorateResult, DecorateWorkerRestart,
    PoolShutdown, DEFAULT_WORKER_WATCHDOG_TIMEOUT,
};
pub use rainbow::{bracket_depths, bracket_ranges, BracketDepth};
pub use sections::{
    heading_at, heading_chain_at, heading_index_at, section_at, section_bounds, SectionBounds,
};
pub use spans::{block_spans, BlockKind, BlockSpan};
pub use syntax::{highlight, HighlightKind, HighlightSpan};
pub use table_eval::{evaluate_tables, EvaluatedTable, TableCellOverride};
pub use table_formula::CellRef;
pub use tables::{column_alignments, TableAlignment};
pub use tree_cache::{BufferTreeCache, CachedBufferTree, BUFFER_TREE_CACHE_CAP};
pub use tree_sitter_alloc::tree_sitter_heap_bytes;
