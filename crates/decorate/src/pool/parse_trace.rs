//! Worker-side parse-path metadata returned alongside every
//! [`crate::DecorateResult`].
//!
//! Pulled out of `pool.rs` so the host file stays under the 600-line
//! conventions cap. The crate root re-exports these types for the
//! public `continuity_decorate::DecorationParseTrace` path.

/// Reason a decoration request used a full parse instead of the incremental
/// tree-sitter path.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DecorationFullParseReason {
    /// No cached tree/revision pair was available for the requested buffer.
    NoPrevTree,
    /// The producer could not cover the gap from the previous decoration
    /// revision to the requested revision.
    CoveredFalse,
    /// The cached source length plus edit shifts did not match the new source
    /// length.
    SanityCheckFailed,
}

impl DecorationFullParseReason {
    /// Stable trace spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoPrevTree => "no_prev_tree",
            Self::CoveredFalse => "covered_false",
            Self::SanityCheckFailed => "sanity_check_failed",
        }
    }
}

/// Parse path metadata for UI trace emission.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DecorationParseTrace {
    /// Decoration was deliberately skipped for a non-Markdown buffer.
    Skipped {
        /// Stable language atom.
        language: &'static str,
        /// Wall-clock microseconds spent before returning empty decorations.
        elapsed_us: u64,
    },
    /// Incremental parse reused a cached tree.
    Incremental {
        /// Number of edit deltas applied to the cached tree.
        delta_count: usize,
        /// Source byte length associated with the cached tree.
        cached_source_len: usize,
        /// Wall-clock microseconds the worker spent on the
        /// `tree.edit` + reparse + extract pipeline.
        elapsed_us: u64,
        /// Wall-clock microseconds spent inside the tree-sitter parse /
        /// reparse step itself, excluding span extraction. Validates
        /// Block 3.1's "tree queries are microseconds" assumption.
        tree_query_us: u64,
        /// Wall-clock microseconds spent translating the parse tree into
        /// the `Decorations` aggregate after the tree was available.
        decoration_compute_us: u64,
    },
    /// Full parse fallback.
    Full {
        /// Fallback reason.
        reason: DecorationFullParseReason,
        /// Wall-clock microseconds the worker spent on the
        /// full parse + extract pipeline.
        elapsed_us: u64,
        /// Wall-clock microseconds spent inside the tree-sitter parse
        /// step itself, excluding span extraction.
        tree_query_us: u64,
        /// Wall-clock microseconds spent translating the parse tree into
        /// the `Decorations` aggregate after the tree was available.
        decoration_compute_us: u64,
    },
}
