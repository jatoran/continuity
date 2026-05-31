//! Decoration request → `Decorations` computation.
//!
//! Split out of `pool.rs` to keep the file under the 600-line cap;
//! contains the worker-side path that picks between incremental and
//! full parse based on `BufferTreeCache` state and request hints, plus
//! the small helpers needed to time and sanity-check the path.
//!
//! **Thread ownership**: invoked from the decoration worker thread that
//! owns the passed-in `BufferTreeCache`. Pure with respect to the
//! `DecorateRequest` — only the cache is mutated.

use crate::decorations::Decorations;
use crate::language::Language;
use crate::pool::parse_trace::{DecorationFullParseReason, DecorationParseTrace};
use crate::pool::DecorateRequest;
use crate::tree_cache::BufferTreeCache;

/// Compute `Decorations` for `req`, preferring the incremental tree-sitter
/// path when the worker's cache holds a tree for `req.prev_revision` and
/// the sanity check passes. Falls back to full parse with a reason tag
/// otherwise.
pub(crate) fn compute_decorations_for_request(
    req: &DecorateRequest,
    tree_cache: Option<&mut BufferTreeCache>,
) -> Option<(Decorations, DecorationParseTrace)> {
    let started = std::time::Instant::now();
    if req.language != Language::Markdown {
        return Some((
            Decorations::empty(req.revision),
            DecorationParseTrace::Skipped {
                language: req.language.as_str(),
                elapsed_us: elapsed_us_since(started),
            },
        ));
    }
    // P17.1 — materialize the rope to a flat `String` exactly once,
    // here on the worker thread, after the latest-wins queue has
    // already coalesced redundant submissions for this buffer. The
    // producer never pays this cost.
    let source: String = req.rope.to_string();
    let source_len = source.len();
    let Some(cache) = tree_cache else {
        let (decorations, _tree, tree_query_us, decoration_compute_us) =
            Decorations::compute_with_tree_split(&source, req.revision)?;
        return Some((
            decorations,
            DecorationParseTrace::Full {
                reason: req.full_parse_reason,
                elapsed_us: elapsed_us_since(started),
                tree_query_us,
                decoration_compute_us,
            },
        ));
    };

    let full_reparse = |cache: &mut BufferTreeCache, reason| {
        let (decorations, tree, tree_query_us, decoration_compute_us) =
            Decorations::compute_with_tree_split(&source, req.revision)?;
        cache.insert(req.buffer_id, req.revision, source_len, tree);
        Some((
            decorations,
            DecorationParseTrace::Full {
                reason,
                elapsed_us: elapsed_us_since(started),
                tree_query_us,
                decoration_compute_us,
            },
        ))
    };

    let Some(prev_revision) = req.prev_revision else {
        return full_reparse(cache, req.full_parse_reason);
    };
    let Some(cached) = cache.get_for_revision(req.buffer_id, prev_revision) else {
        return full_reparse(cache, DecorationFullParseReason::NoPrevTree);
    };
    if cached.revision != prev_revision
        || !is_source_len_consistent(cached.source_len, source_len, &req.deltas_since_prev)
    {
        return full_reparse(cache, DecorationFullParseReason::SanityCheckFailed);
    }

    let inc_started = std::time::Instant::now();
    match Decorations::compute_incremental(
        &source,
        req.revision,
        &cached.tree,
        &req.deltas_since_prev,
        cached.source_len,
    ) {
        Some((decorations, tree)) => {
            // Incremental path measures `tree_query_us` as the full
            // tree.edit + reparse roundtrip; per-step split is not
            // available without modifying `compute_incremental`'s
            // internals, so we attribute the whole call to the parse
            // bucket and set `decoration_compute_us` to 0 (the
            // extracted spans live inside the same call). This is
            // honest about what we can measure today; refining the
            // split is a follow-up.
            let tree_query_us =
                u64::try_from(inc_started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let trace = DecorationParseTrace::Incremental {
                delta_count: req.deltas_since_prev.len(),
                cached_source_len: cached.source_len,
                elapsed_us: elapsed_us_since(started),
                tree_query_us,
                decoration_compute_us: 0,
            };
            cache.insert(req.buffer_id, req.revision, source_len, tree);
            Some((decorations, trace))
        }
        None => full_reparse(cache, DecorationFullParseReason::SanityCheckFailed),
    }
}

fn elapsed_us_since(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn is_source_len_consistent(
    cached_source_len: usize,
    new_source_len: usize,
    deltas: &[crate::RopeEditDeltaWithPoints],
) -> bool {
    let Ok(mut len) = isize::try_from(cached_source_len) else {
        return false;
    };
    for delta in deltas {
        let Some(next) = len.checked_add(delta.delta.shift()) else {
            return false;
        };
        len = next;
    }
    let Ok(new_source_len) = isize::try_from(new_source_len) else {
        return false;
    };
    len == new_source_len
}
