//! Trace emission for the cold-build row-count walker.
//!
//! Split out of [`super::frame_build`] to keep the host file under the
//! 600-line convention cap. Three events land per cold walker run when
//! tracing is on:
//!
//! - `event:row_count_walker_stats` — fast/slow path counts + per-stage
//!   wall times.
//! - `event:row_count_slowest_lines` — top-N slowest individual lines.
//! - `event:dwrite_measure_cache` — DirectWrite measure-cache hit/miss
//!   counts (only when the DirectWrite measurer ran).

use continuity_display_map::WalkerStats;
use continuity_render::DirectWriteCacheStats;

/// Emit the walker-stats family of trace events. No-op when tracing is
/// off. Caller has just finished a `compute_row_index_measured` call;
/// `dwrite_stats` is `None` for the fixed-char-width fallback path.
pub(super) fn emit_walker_stats(stats: &WalkerStats, dwrite_stats: Option<DirectWriteCacheStats>) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    crate::paint_trace::log_event(
        "row_count_walker_stats",
        &format!(
            "lines_total={} lines_folded={} lines_unwrapped={} \
             lines_fastpath_upper_bound={} lines_fastpath_segment_sum={} \
             lines_slowpath={} measure_calls={} segment_build_us={} \
             measure_us={} soft_wrap_walk_us={} fenwick_build_us={} \
             run_cache_hits={} run_cache_misses={} \
             wrap_cache_hits={} wrap_cache_misses={} wrap_profile_hits={} \
             segment_cache_hits={} segment_cache_misses={}",
            stats.lines_total,
            stats.lines_folded,
            stats.lines_unwrapped,
            stats.lines_fastpath_upper_bound,
            stats.lines_fastpath_segment_sum,
            stats.lines_slowpath,
            stats.measure_calls,
            stats.segment_build_us,
            stats.measure_us,
            stats.soft_wrap_walk_us,
            stats.fenwick_build_us,
            stats.run_cache_hits,
            stats.run_cache_misses,
            stats.wrap_cache_hits,
            stats.wrap_cache_misses,
            stats.wrap_profile_hits,
            stats.segment_cache_hits,
            stats.segment_cache_misses,
        ),
    );
    let populated = stats.slowest_lines_len as usize;
    if populated > 0 {
        let mut detail = String::with_capacity(populated * 32);
        for (i, record) in stats.slowest_lines[..populated].iter().enumerate() {
            if i > 0 {
                detail.push(' ');
            }
            detail.push_str(&format!(
                "line={} cost_us={} bytes={}",
                record.line_idx, record.cost_us, record.byte_len,
            ));
        }
        crate::paint_trace::log_event("row_count_slowest_lines", &detail);
    }
    if let Some(dwrite) = dwrite_stats {
        crate::paint_trace::log_event(
            "dwrite_measure_cache",
            &format!(
                "hits={} misses={} layouts_created={}",
                dwrite.hits, dwrite.misses, dwrite.layouts_created
            ),
        );
    }
}
