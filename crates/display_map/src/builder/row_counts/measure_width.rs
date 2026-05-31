//! `measure_width` helper for the row-count walker's slow path.
//!
//! Extracted from `crates/display_map/src/builder/row_counts.rs` to
//! keep the parent file under the 600-line conventions cap once
//! P18.12b's profile fast-path wiring landed. The helper wraps
//! [`WidthMeasure::measure_cached`] with run-cache hit/miss bookkeeping
//! and a bypass branch for callers that supply no `content_stamp`.

use crate::style::SpanStyle;
use crate::wrap::{MeasureCacheStatus, WidthMeasure};

use super::{RowCountCacheContext, WalkerStats};

pub(super) fn measure_width(
    measure: &mut dyn WidthMeasure,
    content_stamp: Option<u64>,
    text: &str,
    style: &SpanStyle,
    cache_context: Option<RowCountCacheContext<'_>>,
    stats: Option<&mut WalkerStats>,
) -> f32 {
    let Some(stamp) = content_stamp.filter(|_| cache_context.is_some()) else {
        return measure.measure(text, style);
    };
    let measured = measure.measure_cached(stamp, text, style);
    if let Some(stats) = stats {
        match measured.cache_status {
            MeasureCacheStatus::Hit => {
                stats.run_cache_hits = stats.run_cache_hits.saturating_add(1);
            }
            MeasureCacheStatus::Miss => {
                stats.run_cache_misses = stats.run_cache_misses.saturating_add(1);
            }
            MeasureCacheStatus::Bypassed => {}
        }
    }
    measured.width_dip
}
