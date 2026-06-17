//! Projection-plan classification. Given the same paint state the
//! inline rebuild path already computes, decide which
//! [`super::ProjectionBuildKind`] this paint should realize.
//!
//! This is the **single source of truth** for the inline path and
//! the worker dispatch. The inline path realizes the kind; the
//! worker submission converts it via
//! [`super::ProjectionBuildKind::to_worker_plan`]. Divergence here
//! would let the worker produce a frame the inline fallback wouldn't
//! have built — even with stamp validation that still wastes the
//! worker thread on irrelevant work.
//!
//! Thread ownership: UI thread of one window. Pure function over the
//! caller-provided inputs; no `Window` access required so the
//! classifier is unit-testable without DirectWrite or HWND state.

use std::ops::Range;
use std::sync::Arc;

use continuity_decorate::Decorations;
use continuity_display_map::RowDirty;
use continuity_render::FrameDisplay;
use continuity_text::RopeEditDelta;
use ropey::Rope;

use super::{
    realized_covers, ProjectionBuildKind, COLD_PARTIAL_MIN_SOURCE_LINES, LARGE_DIRTY_SET_THRESHOLD,
};
use crate::window_paint_selection_reveal::CachedFrameSource;

const PARTIAL_DIRTY_LINE_THRESHOLD_BPS: usize = 2500;
const PARTIAL_DIRTY_RANGE_COUNT: usize = 16;
const PARTIAL_SPLICE_DELTA_THRESHOLD: usize = 64;

/// Pick between [`ProjectionBuildKind::Cold`] and
/// [`ProjectionBuildKind::ColdPartial`] based on the buffer's source-line
/// count. Large buffers route to the P18.5b partial walker so paint
/// returns within ~50 ms instead of synchronously walking the whole
/// document. Small buffers stay on the existing cold path.
///
/// The viewport-source-range heuristic is 1:1 source-line ≈ display-row
/// — on the first paint (no prior row index) we have no way to
/// translate display rows to source lines, so we walk roughly the same
/// range. The partial walker's safety margin absorbs the rounding
/// error from soft-wrapped lines. When soft-wrap rows push the viewport
/// beyond the source-line count, keep a tail source window instead of
/// collapsing to an empty range; an empty first partial frame draws a
/// blank focused pane while the full index catches up.
fn cold_or_cold_partial(rope: &Rope, viewport_rows: &Range<u32>) -> ProjectionBuildKind {
    if should_use_partial(rope) {
        ProjectionBuildKind::ColdPartial {
            viewport_source_range: viewport_source_range(rope, viewport_rows),
            safety_margin: continuity_display_map::PARTIAL_WALK_SAFETY_MARGIN,
        }
    } else {
        ProjectionBuildKind::Cold
    }
}

fn should_use_partial(rope: &Rope) -> bool {
    (rope.len_lines() as u32) > COLD_PARTIAL_MIN_SOURCE_LINES
}

fn viewport_source_range(rope: &Rope, viewport_rows: &Range<u32>) -> Range<u32> {
    let source_lines = rope.len_lines() as u32;
    let start = viewport_rows.start.min(source_lines);
    let end = viewport_rows.end.min(source_lines);
    if start < end || source_lines == 0 {
        return start..end;
    }

    let viewport_height = viewport_rows.end.saturating_sub(viewport_rows.start).max(1);
    let tail_lines = viewport_height.min(source_lines);
    source_lines.saturating_sub(tail_lines)..source_lines
}

fn dirty_ranges_from_lines(lines: &[u32], source_line_count: u32) -> Arc<[Range<u32>]> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    let mut iter = lines
        .iter()
        .copied()
        .filter(|line| *line < source_line_count);
    let Some(first) = iter.next() else {
        return Arc::from(Vec::<Range<u32>>::new());
    };
    let mut start = first;
    let mut end = first.saturating_add(1);
    for line in iter {
        if line == end {
            end = end.saturating_add(1);
        } else {
            ranges.push(start..end);
            start = line;
            end = line.saturating_add(1);
        }
    }
    ranges.push(start..end);
    Arc::from(ranges)
}

fn dirty_range_line_count(ranges: &[Range<u32>]) -> usize {
    ranges
        .iter()
        .map(|range| range.end.saturating_sub(range.start) as usize)
        .sum()
}

fn should_route_dirty_partial(
    rope: &Rope,
    prev: &FrameDisplay,
    dirty_ranges: &[Range<u32>],
) -> bool {
    if !should_use_partial(rope) {
        return false;
    }
    let source_lines = rope.len_lines();
    let dirty_lines = dirty_range_line_count(dirty_ranges);
    prev.row_index().is_partial()
        || dirty_lines > LARGE_DIRTY_SET_THRESHOLD
        || dirty_ranges.len() > PARTIAL_DIRTY_RANGE_COUNT
        || dirty_lines.saturating_mul(10_000)
            > source_lines.saturating_mul(PARTIAL_DIRTY_LINE_THRESHOLD_BPS)
}

fn should_route_splice_partial(
    rope: &Rope,
    prev: &FrameDisplay,
    deltas: &[RopeEditDelta],
    splice_dirty_lines: usize,
) -> bool {
    if !should_use_partial(rope) {
        return false;
    }
    prev.row_index().is_partial()
        || deltas.len() > PARTIAL_SPLICE_DELTA_THRESHOLD
        || splice_dirty_lines > LARGE_DIRTY_SET_THRESHOLD
}

/// Inputs needed to decide the build kind. Caller assembles these
/// from the same paint state the inline path already computes;
/// nothing in here pulls more `Window` data than the old inline tree
/// did.
pub(crate) struct ProjectionClassifyInputs<'a> {
    /// Document identifier used only for trace events.
    pub document: u128,
    /// Rope being projected this paint.
    pub rope: &'a Rope,
    /// Rope revision the paint is targeting.
    pub revision: u64,
    /// Soft-wrap width in DIPs (0 = unwrapped).
    pub wrap_width_dip: u32,
    /// Current decoration snapshot, post-transform.
    pub current_decorations: Option<&'a Decorations>,
    /// Decoration snapshot the last painted frame was built with;
    /// `None` when the previous paint had no decorations or none
    /// have been captured yet.
    pub last_painted_decorations: Option<&'a Decorations>,
    /// Motion-fast-path or prewarm-cache candidate (matches the
    /// inline `cached_frame_display`).
    pub cached_frame: Option<&'a FrameDisplay>,
    /// Where `cached_frame` came from. Selection-reveal-rebuild is
    /// only valid against a `LastPaint` or `MouseHitTest` candidate
    /// (a `Prewarm` hit was already built with the current carets).
    pub cached_frame_source: CachedFrameSource,
    /// Fall-back rebuild source when the cached frame doesn't cover
    /// the viewport (matches the inline `last_painted_frame_display`
    /// field of the pair).
    pub last_painted_frame: Option<&'a FrameDisplay>,
    /// Viewport row range the paint will iterate.
    pub viewport_rows: Range<u32>,
    /// Rope deltas applied strictly after the cached frame's
    /// `rope_revision`. Empty when no rope advance.
    pub rope_deltas: &'a [RopeEditDelta],
    /// `true` when the bounded delta history covered every revision
    /// from the cached frame's `rope_revision` up to `revision`.
    pub rope_history_covered: bool,
    /// Source lines whose markdown reveal can have flipped on a
    /// caret-only move (already sorted/deduped).
    pub selection_reveal_dirty: &'a [u32],
    /// `true` when the **decoration parse content** has advanced
    /// since the previous paint — i.e. the worker delivered a new
    /// `Decorations` (with a higher parse revision) into the
    /// per-buffer cache. Distinct from `decoration_advanced` (which
    /// is derived from the cached frame's stamp): a transformed
    /// stale parse takes the *current* rope revision as its
    /// `Decorations::revision` label, so two paints can share the
    /// same `IndexStamps.decoration_revision` while their underlying
    /// parse content differs. The caller (paint) tracks the
    /// worker's parse revision on `Window::last_painted_decoration_parse_revision`
    /// and sets this flag when it changes. Forces the covering-cache
    /// fast path to fail and routes through the
    /// `decoration_advanced` rebuild branch so the new styling lands.
    pub decoration_parse_advanced: bool,
}

/// Classify how the current paint should build its frame display.
#[must_use]
pub(crate) fn classify_projection_build(
    inputs: ProjectionClassifyInputs<'_>,
) -> ProjectionBuildKind {
    // The decoration revision the *current* paint should produce.
    // Falls back to the rope revision when no decorations are
    // attached, matching the `Decorations::empty(rev)` convention.
    // Hoisted above the covering-cache fast path so the filter can
    // reject a cached frame whose decoration is stale.
    let current_decoration_rev = inputs
        .current_decorations
        .map(|d| d.revision)
        .unwrap_or(inputs.revision);

    // Covering-cache fast path. The cached frame realizes the
    // viewport at the current rope AND decoration revisions; either
    // the caret moved (selection-only reveal rebuild) or nothing
    // relevant changed (true cache hit).
    //
    // The `decoration_revision` check is load-bearing: without it,
    // an async decoration delivery (worker finishes a markdown
    // re-parse) that lands while neither the rope nor the caret has
    // moved produces a `CacheHit` against the stale-decoration
    // frame, silently dropping the new styling on the floor. The
    // user sees raw markers until some unrelated edit invalidates
    // the cache. Confirmed via manual trace
    // `perf-snapshots/manual-lag_after-coalesce_20260517-150127.tsv`
    // where the decoration watchdog tick fired, paint ran, and the
    // markdown styling failed to appear because this filter let
    // through the stale-decoration covering frame.
    let covering = inputs.cached_frame.filter(|cached| {
        let stamps = cached.row_index().stamps();
        realized_covers(cached.realized_row_range(), &inputs.viewport_rows)
            && stamps.rope_revision == inputs.revision
            && stamps.decoration_revision == current_decoration_rev
            && !inputs.decoration_parse_advanced
    });
    if let Some(cached) = covering {
        if matches!(
            inputs.cached_frame_source,
            CachedFrameSource::LastPaint | CachedFrameSource::MouseHitTest
        ) && !inputs.selection_reveal_dirty.is_empty()
        {
            return ProjectionBuildKind::SelectionRebuild {
                prev: cached.clone(),
                dirty: inputs.selection_reveal_dirty.to_vec(),
            };
        }
        if (inputs
            .cached_frame
            .is_some_and(|frame| frame.row_index().is_partial())
            || inputs
                .last_painted_frame
                .is_some_and(|frame| frame.row_index().is_partial()))
            && crate::paint_trace::is_trace_enabled()
        {
            crate::paint_trace::log_event(
                "event:partial_prev_cache_hit_preserved",
                &format!("buffer_id={}", inputs.document),
            );
        }
        return ProjectionBuildKind::CacheHit(cached.clone());
    }

    // The covering cache missed. Either rope/decoration drift, or
    // the realized window doesn't cover the viewport (scroll). Pick
    // the strongest reusable prev frame.
    let rebuild_source = inputs.cached_frame.or(inputs.last_painted_frame);
    let Some(prev) = rebuild_source else {
        return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
    };

    let cached_stamps = prev.row_index().stamps();
    // Soft-wrap / font / fold-shape drift still forces a fresh
    // viewport build — those changes ripple across every line.
    if cached_stamps.wrap_width_dip != inputs.wrap_width_dip {
        return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
    }

    let prev_rope_rev = cached_stamps.rope_revision;
    let prev_decoration_rev = cached_stamps.decoration_revision;
    let rope_advanced = prev_rope_rev < inputs.revision;
    // A change in the **transformed** decoration revision (rope-rev
    // label) or a change in the underlying **parse** content both
    // require a rebuild. The parse-content change is the case the
    // stamp can't see — see `decoration_parse_advanced` docs above.
    let decoration_advanced =
        prev_decoration_rev != current_decoration_rev || inputs.decoration_parse_advanced;

    if !rope_advanced && !decoration_advanced {
        if prev.row_index().is_partial() {
            return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
        }
        // ε.3F++ (2026-05-17): same-revision viewport miss. The
        // realized window doesn't cover the requested viewport but
        // `prev`'s row index is already built against the current
        // rope and decoration revisions — only the new viewport's
        // specs need materialising. Selection-reveal flips
        // (caret-only motion that didn't bump a stamp) ride along
        // as the dirty list so the affected source lines rebuild
        // their reveal state.
        return ProjectionBuildKind::ViewportRealize {
            prev: prev.clone(),
            dirty: inputs.selection_reveal_dirty.to_vec(),
        };
    }

    if rope_advanced && !inputs.rope_history_covered {
        // Bounded delta history dropped what we'd need to transform
        // through. Cold build is the safe ground-truth.
        return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
    }

    if rope_advanced && inputs.rope_deltas.is_empty() {
        // Defense-in-depth: the rope revision advanced but the history
        // reported zero deltas for the span. A revision only advances on
        // a real content edit, so "advanced + no deltas" means we cannot
        // compute a dirty/splice plan — and reusing `prev`'s specs against
        // the changed rope can slice out-of-bounds during paint (the
        // historical undo/redo crash, which failed to record delta
        // history). Cold build is the safe ground truth. With delta
        // history correctly recorded on every edit path this branch is
        // unreachable; it stays as a backstop.
        return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
    }

    let mut combined: Vec<u32> = Vec::new();
    let mut splice_plan = None;

    if !inputs.rope_deltas.is_empty() {
        match prev
            .row_index()
            .dirty_after_rope_edits(inputs.rope_deltas, inputs.rope)
        {
            RowDirty::Lines(lines) => combined.extend(lines),
            RowDirty::Splice(splice) => splice_plan = Some(splice),
            RowDirty::FullRebuild => {
                if crate::paint_trace::is_trace_enabled() {
                    let prev_lines = prev.row_index().source_line_count();
                    let new_lines = inputs.rope.len_lines() as u32;
                    crate::paint_trace::log_event(
                        "event:row_dirty_full_rebuild",
                        &format!(
                            "reason=line_count_changed prev_lines={prev_lines} new_lines={new_lines} deltas={}",
                            inputs.rope_deltas.len(),
                        ),
                    );
                }
                return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
            }
        }
    }

    if let Some(splice) = splice_plan {
        // ε.3F structural splice runs alone — decoration and
        // selection-reveal dirty sets do not ride along the splice
        // path (matches the inline `if let (Some(prev), Some(splice))
        // = ...` early-return shape).
        let splice_dirty_lines = splice.dirty.len();
        let deltas = Arc::from(inputs.rope_deltas.to_vec());
        if should_route_splice_partial(inputs.rope, prev, inputs.rope_deltas, splice_dirty_lines) {
            return ProjectionBuildKind::SplicePartial {
                prev: prev.clone(),
                viewport_source_range: viewport_source_range(inputs.rope, &inputs.viewport_rows),
                safety_margin: continuity_display_map::PARTIAL_WALK_SAFETY_MARGIN,
                deltas,
                splice,
            };
        }
        return ProjectionBuildKind::Splice {
            prev: prev.clone(),
            splice,
            deltas,
        };
    }

    if decoration_advanced {
        match (inputs.last_painted_decorations, inputs.current_decorations) {
            (None, None) => {}
            (Some(prev_decorations), Some(current_decorations)) => {
                if prev_decorations.revision != prev_decoration_rev {
                    return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows);
                }
                let transformed_prev = if inputs.rope_deltas.is_empty() {
                    prev_decorations.clone()
                } else {
                    prev_decorations.transformed_through(inputs.rope_deltas, current_decoration_rev)
                };
                let deco_dirty =
                    current_decorations.diff_dirty_lines(&transformed_prev, inputs.rope);
                combined.extend(deco_dirty);
            }
            _ => return cold_or_cold_partial(inputs.rope, &inputs.viewport_rows),
        }
    }

    if !inputs.selection_reveal_dirty.is_empty() {
        combined.extend(inputs.selection_reveal_dirty.iter().copied());
    }
    combined.sort_unstable();
    combined.dedup();
    let dirty_source_ranges = dirty_ranges_from_lines(&combined, inputs.rope.len_lines() as u32);

    if should_route_dirty_partial(inputs.rope, prev, &dirty_source_ranges) {
        return ProjectionBuildKind::DirtyPartial {
            prev: prev.clone(),
            viewport_source_range: viewport_source_range(inputs.rope, &inputs.viewport_rows),
            safety_margin: continuity_display_map::PARTIAL_WALK_SAFETY_MARGIN,
            dirty_source_ranges,
        };
    }

    ProjectionBuildKind::Dirty {
        prev: prev.clone(),
        dirty: combined,
    }
}

#[cfg(test)]
mod tests;
