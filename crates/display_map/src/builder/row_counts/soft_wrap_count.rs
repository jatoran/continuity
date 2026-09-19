//! Soft-wrap row counting for one source line — the grapheme walk behind
//! [`super::row_count_for_source_line`], split out of `row_counts.rs` so
//! that file stays under the conventions cap. Mirrors the count the full
//! builder emits via `soft_wrap_spec` without materializing a
//! `DisplayLineSpec`.
//!
//! Thread ownership: runs on the same worker as the row-count walker.

use std::time::Instant;

use unicode_segmentation::UnicodeSegmentation;

use crate::error::Error;
use crate::id::SourceByte;
use crate::segment::DisplaySegment;
use crate::style::SpanStyle;
use crate::wrap::{preferred_word_break, WidthMeasure, WrapConfig};
use crate::wrap_cache::WrapCacheKey;

use super::measure_width::measure_width;
use super::wrap_lookup::try_cached_wrap_rows;
use super::{accumulate_stage_us, RowCountCacheContext, SoftWrapRowCount, WalkerStats};

/// Soft-wrap row count for a single source line, mirroring the count
/// the full builder would emit via `soft_wrap_spec` — but without
/// allocating a `DisplayLineSpec` or `Vec<DisplaySegment>` for the
/// output.
///
/// Returns `1` when the segments' summed display width fits within
/// `wrap.width_dip`. Otherwise walks the grapheme cluster sequence
/// once, accumulating break points the same way
/// `grapheme_word_break_points_styled` does, and returns
/// `break_count + 1`.
#[allow(clippy::needless_option_as_deref)]
#[allow(clippy::too_many_arguments)]
pub(super) fn count_soft_wrap_rows(
    segments: &[DisplaySegment],
    line_text: &str,
    source_byte_start: usize,
    wrap: WrapConfig,
    continuation_budget_dip: f32,
    measure: &mut dyn WidthMeasure,
    cache_context: Option<RowCountCacheContext<'_>>,
    content_stamp: Option<u64>,
    mut stats: Option<&mut WalkerStats>,
) -> Result<SoftWrapRowCount, Error> {
    let max_width = wrap.width_dip as f32;
    let source_byte_start_typed = SourceByte::from_usize(source_byte_start);

    // Trivial-fit fast path. For markdown buffers most lines are short
    // ASCII and the per-segment `measure` calls turn into thousands of
    // `IDWriteTextLayout::CreateTextLayout` invocations during the
    // whole-document row-count walk — ~450 ms on a 9 k-line buffer in
    // release builds (`perf-snapshots/manual-lag_after-coalesce_20260517-235814.tsv`).
    // When the upper-bound estimate (byte count × measurer-provided
    // per-byte advance) already fits the wrap width, the line cannot
    // possibly need a second display row regardless of glyph metrics.
    let mut upper_bound_width = 0.0_f32;
    for seg in segments {
        let bytes = seg.display_bytes(line_text, source_byte_start_typed);
        if bytes.is_empty() {
            continue;
        }
        let style = seg.style().copied().unwrap_or_else(SpanStyle::body);
        let max_advance = measure.max_byte_advance(&style);
        if !max_advance.is_finite() {
            upper_bound_width = f32::INFINITY;
            break;
        }
        upper_bound_width += bytes.len() as f32 * max_advance;
        if upper_bound_width > max_width {
            break;
        }
    }
    if upper_bound_width <= max_width {
        if let Some(stats) = stats.as_deref_mut() {
            stats.lines_fastpath_upper_bound = stats.lines_fastpath_upper_bound.saturating_add(1);
        }
        return Ok(SoftWrapRowCount {
            rows: 1,
            should_cache_segments: false,
        });
    }

    // Fast path — sum segment widths. If they fit, the line is one row.
    let mut total_width = 0.0_f32;
    let mut segment_measure_calls: u64 = 0;
    let t_measure = stats.as_deref().map(|_| Instant::now());
    for seg in segments {
        let bytes = seg.display_bytes(line_text, source_byte_start_typed);
        if bytes.is_empty() {
            continue;
        }
        let style = seg.style().copied().unwrap_or_else(SpanStyle::body);
        let w = measure_width(
            measure,
            content_stamp,
            bytes,
            &style,
            cache_context,
            stats.as_deref_mut(),
        );
        segment_measure_calls += 1;
        if !w.is_finite() || w < 0.0 {
            return Err(Error::BadMeasurement(w));
        }
        total_width += w;
    }
    accumulate_stage_us(&mut stats, t_measure, |s| &mut s.measure_us);
    if total_width <= max_width {
        if let Some(stats) = stats.as_deref_mut() {
            stats.lines_fastpath_segment_sum = stats.lines_fastpath_segment_sum.saturating_add(1);
            stats.measure_calls = stats.measure_calls.saturating_add(segment_measure_calls);
        }
        return Ok(SoftWrapRowCount {
            rows: 1,
            should_cache_segments: false,
        });
    }

    if let Some(cached) = try_cached_wrap_rows(
        cache_context,
        content_stamp,
        wrap,
        continuation_budget_dip,
        stats.as_deref_mut(),
        segment_measure_calls,
    ) {
        return Ok(cached);
    }
    if cache_context.is_some() && content_stamp.is_some() {
        if let Some(stats) = stats.as_deref_mut() {
            stats.wrap_cache_misses = stats.wrap_cache_misses.saturating_add(1);
        }
    }

    // Slow path — count break points the same way the full builder
    // does. Mirrors `grapheme_word_break_points_styled` line-by-line so
    // the count never disagrees with what the realized vec would
    // contain.
    //
    // `t_slowpath` times the *entire* slow-path block (graph walking +
    // per-grapheme measure + break-point bookkeeping). The per-grapheme
    // measure calls are also counted in `measure_calls`; their wall-time
    // contribution stays inside `soft_wrap_walk_us`, so `measure_us`
    // (fast-path-only) and `soft_wrap_walk_us` are non-overlapping wall-
    // clock buckets.
    //
    // P18.12a (2026-05-22) — alongside the existing `breaks` counter
    // we accumulate a width-independent line-wrap profile: cumulative
    // width from line start at every whitespace break candidate, split
    // into "pre-whitespace" (just before the trailing whitespace) and
    // "post-whitespace" (including the trailing whitespace). The
    // `running` accumulator is row-relative (resets at cuts); the
    // profile accumulator `cum_from_line_start` is line-relative and
    // never resets. Both update once per grapheme — no extra
    // DirectWrite calls. See `WrapCacheEntry` doc in `wrap_cache.rs`
    // and `crate::wrap_profile` for the consumer contract.
    let t_slowpath = stats.as_deref().map(|_| Instant::now());
    let mut breaks: u16 = 0;
    let mut last_cut: Option<usize> = None;
    let mut line_start_byte = 0_usize;
    let mut last_word_break: Option<usize> = None;
    // `running_at_word_break` mirrors the exact carry-over fix in
    // `grapheme_word_break_points_styled` so the walker's row *count* stays
    // consistent with the painted break *positions* on multi-segment lines.
    let mut running = 0.0_f32;
    let mut running_at_word_break = 0.0_f32;
    let mut segment_base = 0_usize;
    let mut grapheme_measure_calls: u64 = 0;
    let mut cum_from_line_start = 0.0_f32;
    let mut break_offsets: Vec<u32> = Vec::new();
    let mut prefix_advances_bits: Vec<u32> = Vec::new();
    let mut pre_whitespace_advances_bits: Vec<u32> = Vec::new();
    // First row budgets the full wrap width; continuation rows budget
    // the hang-indent-reduced width (mirrors
    // `grapheme_word_break_points_styled`).
    let mut row_budget = max_width;
    let protected_list_prefix_end =
        super::list_prefix::compute_list_prefix_end(segments, line_text, source_byte_start_typed);
    for seg in segments {
        let bytes = seg.display_bytes(line_text, source_byte_start_typed);
        if bytes.is_empty() {
            continue;
        }
        let style = seg.style().copied().unwrap_or_else(SpanStyle::body);
        for (rel_off, g) in bytes.grapheme_indices(true) {
            let byte_off = segment_base + rel_off;
            let w = measure_width(
                measure,
                content_stamp,
                g,
                &style,
                cache_context,
                stats.as_deref_mut(),
            );
            grapheme_measure_calls += 1;
            let is_whitespace = g.chars().any(|c| c.is_whitespace());
            if is_whitespace {
                // Record the break candidate with both pre- and post-
                // whitespace cumulative widths. `cum_from_line_start`
                // is line-relative and increments below regardless of
                // the cut decision.
                let break_offset = (byte_off + g.len()) as u32;
                pre_whitespace_advances_bits.push(cum_from_line_start.to_bits());
                let post_whitespace_advance = cum_from_line_start + w;
                prefix_advances_bits.push(post_whitespace_advance.to_bits());
                break_offsets.push(break_offset);
                last_word_break = Some(byte_off + g.len());
                running_at_word_break = running + w;
            }
            cum_from_line_start += w;
            if running + w > row_budget && byte_off > line_start_byte {
                let word_break = preferred_word_break(
                    last_word_break,
                    line_start_byte,
                    byte_off,
                    protected_list_prefix_end,
                );
                let cut = word_break.unwrap_or(byte_off);
                breaks = breaks.saturating_add(1);
                last_cut = Some(cut);
                line_start_byte = cut;
                row_budget = continuation_budget_dip;
                // Exact carry-over with no re-measure (matches
                // `grapheme_word_break_points_styled`): a word-boundary break
                // carries everything past the break point; a hard grapheme
                // break starts the new row at the current grapheme.
                running = match word_break {
                    Some(_) => (running + w - running_at_word_break).max(0.0),
                    None => w,
                };
                last_word_break = None;
                running_at_word_break = 0.0;
            } else {
                running += w;
            }
        }
        segment_base += bytes.len();
    }

    let rows = super::row_result::compute_materialized_rows(breaks, last_cut, segment_base);
    accumulate_stage_us(&mut stats, t_slowpath, |s| &mut s.soft_wrap_walk_us);
    if let Some(stats) = stats.as_deref_mut() {
        stats.lines_slowpath = stats.lines_slowpath.saturating_add(1);
        // Include the segment-sum probe calls plus the grapheme + suffix calls.
        let total_calls = segment_measure_calls.saturating_add(grapheme_measure_calls);
        stats.measure_calls = stats.measure_calls.saturating_add(total_calls);
    }

    // Append the end-of-line sentinel when the line does not already
    // end with a whitespace break candidate. The sentinel's pre- and
    // post-whitespace advances are equal (no trailing whitespace), so
    // `wrap_profile` distinguishes it from a real whitespace break by
    // that equality.
    let line_end_offset = segment_base as u32;
    if break_offsets.last() != Some(&line_end_offset) {
        break_offsets.push(line_end_offset);
        prefix_advances_bits.push(cum_from_line_start.to_bits());
        pre_whitespace_advances_bits.push(cum_from_line_start.to_bits());
    }

    if let (Some(ctx), Some(stamp)) = (cache_context, content_stamp) {
        let key = WrapCacheKey::new(stamp, ctx.font_state, ctx.locale, wrap.width_dip);
        ctx.wrap_cache.insert(
            key,
            crate::wrap_cache::WrapCacheEntry {
                row_count: rows,
                break_points: break_offsets.into(),
                prefix_advances_bits: prefix_advances_bits.into(),
                pre_whitespace_advances_bits: pre_whitespace_advances_bits.into(),
            },
        );
    }

    Ok(SoftWrapRowCount {
        rows,
        should_cache_segments: true,
    })
}
