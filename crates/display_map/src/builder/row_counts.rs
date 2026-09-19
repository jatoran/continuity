//! ε.2 — cheap row-count walker.
//!
//! Counts how many display rows each source line projects to, given the
//! current rope / decoration / fold / image-reservation / soft-wrap
//! state, without materializing any [`crate::DisplayLineSpec`].
//! Drives the `DisplayRowIndex` that
//! [`crate::DisplayMapBuilder::build_viewport`] consumes to decide
//! which source lines to materialize.
//!
//! ## Per-walk statistics
//!
//! Callers pass an optional [`WalkerStats`] accumulator so paint-time
//! tracing can attribute the walker's cost to the right sub-step
//! (upper-bound fast path, segment-sum fast path, grapheme slow path).
//! When `None`, the walker pays only one branch per line for the check
//! and produces no measurement overhead.
//!
//! ## Thread ownership
//!
//! Runs on the same worker as [`crate::DisplayMapBuilder::build_viewport`].

use std::time::Instant;

use continuity_buffer::RopeSnapshot;
use continuity_decorate::Decorations;

use crate::error::Error;
use crate::fold::FoldRange;
use crate::id::SourceByte;
use crate::image_row_reservation_provider::ImageRowReservation;
use crate::markdown_toggles::MarkdownRenderToggles;
use crate::wrap::{continuation_wrap_budget_dip, hanging_indent_dip, WidthMeasure, WrapConfig};
use crate::wrap_cache::WrapCache;
use crate::{compute_line_projection_stamp, SegmentCache, SegmentCacheKey};

use super::line_helpers::{line_is_hidden, read_line_text, source_line_range};
use super::segments::build_line_segments;
use super::stats::record_slowest_line;
use super::{SlowestLineRecord, WalkerStats};

/// Shared row-count cache context for one walker invocation.
#[derive(Clone, Copy)]
pub(super) struct RowCountCacheContext<'a> {
    pub(super) font_state: u64,
    pub(super) locale: &'a str,
    pub(super) wrap_cache: &'a WrapCache,
    pub(super) segment_cache: &'a SegmentCache,
}

struct SoftWrapRowCount {
    rows: u16,
    should_cache_segments: bool,
}

/// Add `t0.elapsed()` (in microseconds, saturating) to `*field` if both
/// `stats` and `t0` are populated. Inlined so the no-trace path
/// (`stats == None`, `t0 == None`) optimises to zero instructions
/// beyond two checks.
#[inline]
pub(super) fn accumulate_stage_us(
    stats: &mut Option<&mut WalkerStats>,
    t0: Option<Instant>,
    field: impl Fn(&mut WalkerStats) -> &mut u64,
) {
    if let (Some(s), Some(t0)) = (stats.as_deref_mut(), t0) {
        let us = t0.elapsed().as_micros() as u64;
        let slot = field(s);
        *slot = slot.saturating_add(us);
    }
}

/// Walk every source line and return the per-line display-row counts.
///
/// Folded source lines contribute `0`. Image-reservation phantom rows
/// inflate the count to at least `reserved_display_rows` (matching the
/// full builder's reservation-cursor logic).
///
/// When `stats` is `Some`, the walker increments the accumulator at
/// each decision point so the caller can emit a `paint:row_count_walker_stats`
/// trace alongside the outer build span.
// `stats.as_deref_mut()` is the idiomatic reborrow for
// `Option<&mut WalkerStats>` inside a loop body — clippy's
// `needless_option_as_deref` flag fires because the deref'd type
// matches the input, but the LIFETIME is what's actually being
// rebound. Allow the lint for the walker helpers below.
#[allow(clippy::too_many_arguments, clippy::needless_option_as_deref)]
pub(super) fn compute_row_counts(
    snapshot: &RopeSnapshot,
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    markdown_toggles: MarkdownRenderToggles,
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    mut stats: Option<&mut WalkerStats>,
) -> Result<Vec<u16>, Error> {
    let rope = snapshot.rope();
    let source_line_count = rope.len_lines() as u32;
    let mut row_counts: Vec<u16> = vec![0u16; source_line_count as usize];
    let mut reservation_cursor: usize = 0;

    for source_line_idx in 0..source_line_count {
        if let Some(stats) = stats.as_deref_mut() {
            stats.lines_total = stats.lines_total.saturating_add(1);
        }
        // Step the reservation cursor in lockstep with the loop so the
        // shared `row_count_for_source_line` helper finds the
        // applicable reservation in O(1).
        while reservation_cursor < image_reservations.len()
            && image_reservations[reservation_cursor].source_line.raw() < source_line_idx
        {
            reservation_cursor += 1;
        }
        let t_line = stats.as_deref().map(|_| Instant::now());
        let count = row_count_for_source_line(
            rope,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            suppressed_table_blocks,
            markdown_toggles,
            wrap,
            measure,
            cache_context,
            source_line_idx,
            reservation_cursor,
            stats.as_deref_mut(),
        )?;
        if let (Some(stats), Some(t0)) = (stats.as_deref_mut(), t_line) {
            let cost_us = u32::try_from(t0.elapsed().as_micros()).unwrap_or(u32::MAX);
            let (start, end) = source_line_range(rope, source_line_idx as usize);
            let byte_len = u32::try_from(end.saturating_sub(start)).unwrap_or(u32::MAX);
            record_slowest_line(
                stats,
                SlowestLineRecord {
                    line_idx: source_line_idx,
                    cost_us,
                    byte_len,
                },
            );
        }
        row_counts[source_line_idx as usize] = count;
        if image_reservations
            .get(reservation_cursor)
            .is_some_and(|r| r.source_line.raw() == source_line_idx)
        {
            reservation_cursor += 1;
        }
    }

    Ok(row_counts)
}

/// ε.3 — row count for a single source line. Used both inside
/// `compute_row_counts` (whole-document walk) and by
/// `DisplayMapBuilder::rebuild_dirty` to refresh the row count for
/// each dirty source line without scanning the rest of the document.
///
/// `reservation_cursor` is the caller's position into
/// `image_reservations`; the helper reads it but does not advance it.
#[allow(clippy::too_many_arguments, clippy::needless_option_as_deref)]
pub(super) fn row_count_for_source_line(
    rope: &ropey::Rope,
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    suppressed_table_blocks: &[std::ops::Range<usize>],
    markdown_toggles: MarkdownRenderToggles,
    wrap: WrapConfig,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    source_line_idx: u32,
    reservation_cursor: usize,
    mut stats: Option<&mut WalkerStats>,
) -> Result<u16, Error> {
    let (line_start, line_end) = source_line_range(rope, source_line_idx as usize);
    let line_text = read_line_text(rope, line_start, line_end);

    if line_is_hidden(folds, &line_text, line_start, line_end) {
        if let Some(stats) = stats.as_deref_mut() {
            stats.lines_folded = stats.lines_folded.saturating_add(1);
        }
        return Ok(0);
    }

    // Continuation rows are painted shifted right by the line's hanging
    // indent, so every wrap-row decision below budgets them at the
    // reduced width. Cheap: `hanging_indent_dip` returns 0.0 without
    // measuring for the common no-indent / no-marker line, and the two
    // probe measurements (" " and "\t") are served from the measurer's
    // single-ASCII cache.
    let continuation_budget_dip = if wrap.enabled() {
        continuation_wrap_budget_dip(
            wrap.width_dip as f32,
            hanging_indent_dip(&line_text, measure),
        )
    } else {
        0.0
    };

    let natural_rows: u16 = if wrap.enabled()
        && crate::table_hide_provider::is_rendered_table_line(
            decorations,
            suppressed_table_blocks,
            line_start,
            line_end,
        ) {
        // Rendered table rows never soft-wrap: the chrome reserves the
        // rows it needs via `image_reservations` (merged below). See
        // `table_hide_provider::is_rendered_table_line`.
        1
    } else if wrap.enabled() {
        // P18.12d (2026-05-22) — `content_stamp` and `cache_context`
        // are now passed to `count_soft_wrap_rows` *regardless* of
        // `line_text.is_ascii()`, so the wrap cache and P18.12b
        // profile fast path serve ASCII content too. The original
        // `is_ascii` filter was added to bound *segment*-cache
        // memory (segments for ASCII lines are cheap to rebuild and
        // each entry's ~16-segment vec is heavier than a wrap-cache
        // profile); that filter still applies to `segment_key` below.
        // Without this decoupling, English-text buffers (e.g. the
        // dev log) bypassed the entire P18.12 cache machinery and
        // every drag tick re-ran the slow walker — see
        // `P18.12d_extend_wrap_cache_to_ascii_*.md`.
        let content_stamp = cache_context.map(|_| {
            compute_line_projection_stamp(
                decorations,
                caret_bytes,
                folds,
                suppressed_table_blocks,
                markdown_toggles,
                line_start,
                line_end,
                &line_text,
            )
        });
        if let Some(cached) = try_cached_wrap_rows(
            cache_context,
            content_stamp,
            wrap,
            continuation_budget_dip,
            stats.as_deref_mut(),
            0,
        ) {
            cached.rows
        } else {
            let segment_cache_eligible = !line_text.is_ascii();
            let segment_key = if segment_cache_eligible {
                cache_context
                    .zip(content_stamp)
                    .map(|(ctx, stamp)| SegmentCacheKey::new(stamp, ctx.font_state))
            } else {
                None
            };
            let mut segment_cache_missed = false;
            let segments = if let (Some(ctx), Some(key)) = (cache_context, segment_key) {
                if let Some(cached) = ctx.segment_cache.get_shifted(&key, line_start) {
                    if let Some(stats) = stats.as_deref_mut() {
                        stats.segment_cache_hits = stats.segment_cache_hits.saturating_add(1);
                    }
                    cached
                } else {
                    segment_cache_missed = true;
                    if let Some(stats) = stats.as_deref_mut() {
                        stats.segment_cache_misses = stats.segment_cache_misses.saturating_add(1);
                    }
                    let t_segments = stats.as_deref().map(|_| Instant::now());
                    let built = build_line_segments(
                        decorations,
                        caret_bytes,
                        folds,
                        suppressed_table_blocks,
                        markdown_toggles,
                        line_start,
                        line_end,
                        &line_text,
                    );
                    accumulate_stage_us(&mut stats, t_segments, |s| &mut s.segment_build_us);
                    built
                }
            } else {
                let t_segments = stats.as_deref().map(|_| Instant::now());
                let built = build_line_segments(
                    decorations,
                    caret_bytes,
                    folds,
                    suppressed_table_blocks,
                    markdown_toggles,
                    line_start,
                    line_end,
                    &line_text,
                );
                accumulate_stage_us(&mut stats, t_segments, |s| &mut s.segment_build_us);
                built
            };
            let counted = count_soft_wrap_rows(
                &segments,
                &line_text,
                line_start,
                wrap,
                continuation_budget_dip,
                measure,
                cache_context,
                content_stamp,
                stats.as_deref_mut(),
            )?;
            if counted.should_cache_segments && segment_cache_missed {
                if let (Some(ctx), Some(key)) = (cache_context, segment_key) {
                    ctx.segment_cache.insert(key, line_start, &segments);
                }
            }
            counted.rows
        }
    } else {
        if let Some(stats) = stats.as_deref_mut() {
            stats.lines_unwrapped = stats.lines_unwrapped.saturating_add(1);
        }
        1
    };

    let mut total = natural_rows;
    if let Some(reservation) = image_reservations.get(reservation_cursor) {
        if reservation.source_line.raw() == source_line_idx && natural_rows > 0 {
            let target = u16::try_from(reservation.reserved_display_rows).unwrap_or(u16::MAX);
            if target > total {
                total = target;
            }
        }
    }
    Ok(total)
}

mod list_prefix;
mod measure_width;
mod row_result;
mod soft_wrap_count;
mod wrap_lookup;
use soft_wrap_count::count_soft_wrap_rows;
use wrap_lookup::try_cached_wrap_rows;

#[cfg(test)]
mod tests;
