//! Exact/profile wrap-row lookup shared by early and late walker paths.
//!
//! Kept outside `row_counts.rs` so the hot-path fix does not push the
//! parent file past the 600-line conventions cap.

use crate::wrap::WrapConfig;
use crate::wrap_cache::WrapCacheKey;

use super::{RowCountCacheContext, SoftWrapRowCount, WalkerStats};

pub(super) fn try_cached_wrap_rows(
    cache_context: Option<RowCountCacheContext<'_>>,
    content_stamp: Option<u64>,
    wrap: WrapConfig,
    stats: Option<&mut WalkerStats>,
    measure_calls_on_hit: u64,
) -> Option<SoftWrapRowCount> {
    let (Some(ctx), Some(stamp)) = (cache_context, content_stamp) else {
        return None;
    };
    let key = WrapCacheKey::new(stamp, ctx.font_state, ctx.locale, wrap.width_dip);
    if let Some(entry) = ctx.wrap_cache.get(&key) {
        record_wrap_hit(stats, |s| &mut s.wrap_cache_hits, measure_calls_on_hit);
        return Some(SoftWrapRowCount {
            rows: entry.row_count,
            should_cache_segments: true,
        });
    }
    if let Some(rows) = crate::wrap_profile::try_serve_via_profile(
        ctx.wrap_cache,
        stamp,
        ctx.font_state,
        ctx.locale,
        wrap.width_dip,
        key,
    ) {
        record_wrap_hit(stats, |s| &mut s.wrap_profile_hits, measure_calls_on_hit);
        return Some(SoftWrapRowCount {
            rows,
            should_cache_segments: true,
        });
    }
    None
}

fn record_wrap_hit(
    stats: Option<&mut WalkerStats>,
    field: impl Fn(&mut WalkerStats) -> &mut u64,
    measure_calls_on_hit: u64,
) {
    if let Some(stats) = stats {
        let slot = field(stats);
        *slot = slot.saturating_add(1);
        stats.lines_slowpath = stats.lines_slowpath.saturating_add(1);
        stats.measure_calls = stats.measure_calls.saturating_add(measure_calls_on_hit);
    }
}
