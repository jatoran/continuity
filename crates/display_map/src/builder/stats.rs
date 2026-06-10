//! Row-count walker statistics types.
//!
//! Kept beside the walker so `builder.rs` stays under the file-length
//! convention while preserving the public `continuity_display_map::WalkerStats`
//! re-export.

/// One slow-line record in [`WalkerStats::slowest_lines`]. Records the
/// per-line walker cost and the line's byte length so a trace consumer
/// can navigate straight to the offending source line without parsing
/// the whole rope.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct SlowestLineRecord {
    /// Zero-based source line index.
    pub line_idx: u32,
    /// Microseconds spent inside `row_count_for_source_line` for this
    /// line.
    pub cost_us: u32,
    /// Line's byte length (excluding trailing newline).
    pub byte_len: u32,
}

/// Capacity of the inline slowest-lines reservoir. Eight gives enough
/// signal to spot patterns (e.g. "all top lines are the same long
/// paragraph") without ballooning `WalkerStats`. Sorted descending by
/// `cost_us`; unused slots are `SlowestLineRecord::default()`.
pub const WALKER_SLOWEST_LINES_CAPACITY: usize = 8;

/// Per-walk accumulator for paint-time tracing. Populated by the cheap
/// row-count walker when the caller passes `Some` to
/// [`super::DisplayMapBuilder::compute_row_index_with_stats`] or
/// [`super::DisplayMapBuilder::build_viewport_with_stats`]. Zero overhead
/// when `None`: each counter increment is gated behind a single
/// `Option::is_some` check.
///
/// Fields name the *decision path* a source line took, not raw runtime.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct WalkerStats {
    /// Total source lines walked.
    pub lines_total: u32,
    /// Source lines fully folded.
    pub lines_folded: u32,
    /// Wrap disabled.
    pub lines_unwrapped: u32,
    /// Wrap enabled; fit via the cheap upper-bound estimate.
    pub lines_fastpath_upper_bound: u32,
    /// Wrap enabled; fit via summed segment widths.
    pub lines_fastpath_segment_sum: u32,
    /// Wrap enabled; slow grapheme-cluster break walk.
    pub lines_slowpath: u32,
    /// Total `WidthMeasure::measure` calls across all lines walked.
    pub measure_calls: u64,
    /// Cumulative microseconds spent inside `build_line_segments`.
    pub segment_build_us: u64,
    /// Cumulative microseconds spent inside the fast-path segment-sum
    /// measure loop.
    pub measure_us: u64,
    /// Cumulative microseconds spent inside the whole grapheme-cluster
    /// slow path.
    pub soft_wrap_walk_us: u64,
    /// Microseconds spent building the Fenwick prefix-sum tree.
    pub fenwick_build_us: u64,
    /// Shared run-cache hits while measuring slow-path fragments.
    pub run_cache_hits: u64,
    /// Shared run-cache misses while measuring slow-path fragments.
    pub run_cache_misses: u64,
    /// Shared wrap-cache hits for slow-path row counts.
    pub wrap_cache_hits: u64,
    /// Shared wrap-cache misses for slow-path row counts.
    pub wrap_cache_misses: u64,
    /// Width-independent profile hits served via
    /// [`crate::wrap_profile::row_count_from_profile`]. Counts lines
    /// that missed the exact-width [`crate::WrapCache`] lookup but
    /// found a sibling-bucket entry whose populated profile suffices
    /// to derive `row_count` at the queried `wrap_width_dip` without
    /// re-running the slow walker. P18.12b.
    pub wrap_profile_hits: u64,
    /// Shared segment-cache hits for slow-path segment lists.
    pub segment_cache_hits: u64,
    /// Shared segment-cache misses for slow-path segment lists.
    pub segment_cache_misses: u64,
    /// Inline reservoir of the slowest source lines seen by the walker,
    /// sorted descending by `cost_us`.
    pub slowest_lines: [SlowestLineRecord; WALKER_SLOWEST_LINES_CAPACITY],
    /// Number of populated entries in `slowest_lines`.
    pub slowest_lines_len: u8,
}

/// Insert one observation into the fixed-capacity slowest-lines
/// reservoir on `WalkerStats`. Maintains descending sort order. O(8).
#[inline]
pub(super) fn record_slowest_line(stats: &mut WalkerStats, record: SlowestLineRecord) {
    let len = stats.slowest_lines_len as usize;
    let cap = WALKER_SLOWEST_LINES_CAPACITY;
    if len < cap {
        let pos = stats.slowest_lines[..len]
            .iter()
            .position(|r| r.cost_us < record.cost_us)
            .unwrap_or(len);
        let mut i = len;
        while i > pos {
            stats.slowest_lines[i] = stats.slowest_lines[i - 1];
            i -= 1;
        }
        stats.slowest_lines[pos] = record;
        stats.slowest_lines_len += 1;
    } else if record.cost_us > stats.slowest_lines[cap - 1].cost_us {
        let pos = stats.slowest_lines[..cap]
            .iter()
            .position(|r| r.cost_us < record.cost_us)
            .unwrap_or(cap - 1);
        let mut i = cap - 1;
        while i > pos {
            stats.slowest_lines[i] = stats.slowest_lines[i - 1];
            i -= 1;
        }
        stats.slowest_lines[pos] = record;
    }
}
