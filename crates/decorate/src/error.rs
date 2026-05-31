//! Errors for the `continuity-decorate` crate.

use thiserror::Error;

/// Errors that can arise during decoration computation.
#[derive(Debug, Error)]
pub enum Error {
    /// Tree-sitter failed to load or apply a grammar.
    #[error("tree-sitter language load failed: {0}")]
    LanguageLoad(String),

    /// A decoration result was computed against a revision that has since
    /// advanced. Discard the result.
    #[error("stale decoration revision {0}")]
    StaleRevision(u64),

    /// Decoration computation panicked. The worker catches this per
    /// request, reports the revision as failed, and keeps processing
    /// later decoration work.
    #[error("decoration worker panicked: {0}")]
    WorkerPanic(String),
}
