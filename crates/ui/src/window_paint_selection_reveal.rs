//! Selection-only markdown reveal helpers used by
//! [`crate::Window::on_paint`].
//!
//! When the caret moves without a rope edit, the prior painted
//! `FrameDisplay` is reused via the motion-compatibility fast path
//! (`PrewarmQuery::is_compatible_for_motion` ignores caret bytes).
//! That reuse is wrong for markdown reveal: a line under the new
//! caret must show raw markers, and the line the caret just left
//! must re-hide them. This module computes the minimal source-line
//! dirty set for that transition so the cache-hit path can refresh
//! only the affected lines instead of either painting stale or cold-
//! building the viewport.
//!
//! See `.docs/design/performance.md` and
//! `.docs/development/roadmap_v4.md` (Phase ε.3) for the broader
//! dirty-rebuild contract — this helper is the selection-only sibling
//! to the rope-driven and decoration-driven dirty sets that already
//! feed `DisplayMapBuilder::rebuild_dirty`.
//!
//! Thread ownership: UI thread of one window. Pure functions; reads
//! only its arguments.

/// Where a paint-time cache hit for the frame-display came from.
///
/// `LastPaint` matches via [`crate::display_prewarm_cache::PrewarmQuery::is_compatible_for_motion`]
/// which intentionally ignores caret bytes — so the cached frame
/// may have been built with stale carets and need a selection-only
/// reveal rebuild before reuse. `Prewarm` hits require exact caret
/// bytes (see `PrewarmedDisplayMap::matches_projection`) so their
/// reveal state is already current. `MouseHitTest` follows
/// `LastPaint`: it can be reused across a caret drift that happened
/// between the click hit-test and the following paint.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum CachedFrameSource {
    LastPaint,
    MouseHitTest,
    Prewarm,
    None,
}

/// Compute the source-line dirty set for a selection-only caret move
/// across the same rope/decorations/wrap/font geometry.
///
/// Returns the union of source lines containing any old or new caret,
/// every source line inside any pipe table whose raw-vs-visual reveal
/// state flipped, plus every source line inside a code block whose
/// [`continuity_decorate::BlockKind::FencedCodeBlock`] /
/// [`continuity_decorate::BlockKind::IndentedCodeBlock`] unit-reveal
/// containment flipped between the two caret-byte sets.
///
/// Caller is expected to skip the call when both caret-byte sets are
/// equal; the helper returns an empty Vec in that case as a safety
/// net. The output is sorted and deduplicated, matching
/// [`crate::Window::rebuild_frame_display_dirty`]'s contract.
#[must_use]
pub(crate) fn compute_selection_reveal_dirty_lines(
    rope: &ropey::Rope,
    decorations: Option<&continuity_decorate::Decorations>,
    old_carets: &[usize],
    new_carets: &[usize],
) -> Vec<u32> {
    use continuity_decorate::BlockKind;
    if old_carets == new_carets {
        return Vec::new();
    }
    let len_bytes = rope.len_bytes();
    let total_lines = rope.len_lines().max(1);
    let line_for_byte = |byte: usize| -> u32 {
        let b = byte.min(len_bytes);
        rope.byte_to_line(b).min(total_lines - 1) as u32
    };
    let mut dirty: Vec<u32> = Vec::with_capacity(old_carets.len() + new_carets.len());
    for &b in old_carets {
        dirty.push(line_for_byte(b));
    }
    for &b in new_carets {
        dirty.push(line_for_byte(b));
    }
    if let Some(decorations) = decorations {
        for table in &decorations.evaluated_tables {
            let had = old_carets
                .iter()
                .any(|&b| b >= table.block_range.start && b < table.block_range.end);
            let now = new_carets
                .iter()
                .any(|&b| b >= table.block_range.start && b < table.block_range.end);
            if had == now {
                continue;
            }
            let first = line_for_byte(table.block_range.start);
            let last_byte = table
                .block_range
                .end
                .saturating_sub(1)
                .max(table.block_range.start);
            let last = line_for_byte(last_byte);
            for line in first..=last {
                dirty.push(line);
            }
        }
        for block in &decorations.blocks {
            let reveals_as_unit = matches!(
                block.kind,
                BlockKind::FencedCodeBlock | BlockKind::IndentedCodeBlock
            );
            if !reveals_as_unit {
                continue;
            }
            let had = old_carets
                .iter()
                .any(|&b| b >= block.start_byte && b <= block.end_byte);
            let now = new_carets
                .iter()
                .any(|&b| b >= block.start_byte && b <= block.end_byte);
            if had == now {
                continue;
            }
            let first = line_for_byte(block.start_byte);
            let last_byte = block.end_byte.saturating_sub(1).max(block.start_byte);
            let last = line_for_byte(last_byte);
            for line in first..=last {
                dirty.push(line);
            }
        }
    }
    dirty.sort_unstable();
    dirty.dedup();
    dirty
}

/// Emit the `paint:frame_display:selection_reveal_rebuild` trace
/// event when paint trace is enabled. Extracted from
/// [`crate::Window::on_paint`] to keep the paint dispatcher under the
/// 600-line conventions cap; the detail string lists the dirty source-
/// line count, span, viewport row range, and both caret-byte sets.
pub(crate) fn log_selection_reveal_rebuild(
    dirty: &[u32],
    viewport_rows: &std::ops::Range<u32>,
    old_carets: &[usize],
    new_carets: &[usize],
) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    let lo = dirty.first().copied().unwrap_or(0);
    let hi = dirty.last().copied().unwrap_or(0);
    let detail = format!(
        "dirty_count={} dirty_span={lo}..={hi} viewport={}..{} old_carets={old_carets:?} new_carets={new_carets:?}",
        dirty.len(),
        viewport_rows.start,
        viewport_rows.end,
    );
    crate::paint_trace::log_event("paint:frame_display:selection_reveal_rebuild", &detail);
}

#[cfg(test)]
mod tests {
    use super::compute_selection_reveal_dirty_lines;
    use continuity_decorate::Decorations;
    use ropey::Rope;

    #[test]
    fn identical_carets_yield_no_dirty_lines() {
        let rope = Rope::from_str("alpha\nbeta\n");
        let dirty = compute_selection_reveal_dirty_lines(&rope, None, &[0], &[0]);
        assert!(dirty.is_empty());
    }

    #[test]
    fn caret_move_across_lines_dirties_both_lines() {
        let rope = Rope::from_str("alpha\nbeta\ngamma\n");
        let line0 = rope.line_to_byte(0);
        let line2 = rope.line_to_byte(2);
        let dirty = compute_selection_reveal_dirty_lines(&rope, None, &[line0], &[line2]);
        assert_eq!(dirty, vec![0, 2]);
    }

    #[test]
    fn caret_leaving_fenced_code_block_dirties_every_block_line() {
        let source = "before\n```\nfn main() {}\nlet x = 1;\n```\nafter\n";
        let rope = Rope::from_str(source);
        let decorations = Decorations::compute(source, 1).unwrap();
        let inside = rope.line_to_byte(2) + 1;
        let outside = 0;
        let dirty =
            compute_selection_reveal_dirty_lines(&rope, Some(&decorations), &[inside], &[outside]);
        assert!(dirty.contains(&0), "old caret line dirty: {dirty:?}");
        assert!(dirty.contains(&1), "fence open dirty: {dirty:?}");
        assert!(dirty.contains(&2), "code-body line dirty: {dirty:?}");
        assert!(dirty.contains(&3), "code-body line dirty: {dirty:?}");
        assert!(dirty.contains(&4), "fence close dirty: {dirty:?}");
    }

    #[test]
    fn caret_moving_within_a_line_still_dirties_that_line() {
        let rope = Rope::from_str("**hello world**\nsecond\n");
        let dirty = compute_selection_reveal_dirty_lines(&rope, None, &[2], &[8]);
        assert_eq!(dirty, vec![0]);
    }

    #[test]
    fn caret_leaving_pipe_table_dirties_entire_table_block() {
        let source = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nafter\n";
        let rope = Rope::from_str(source);
        let decorations = Decorations::compute(source, 1).unwrap();
        let inside = rope.line_to_byte(2) + 2;
        let outside = rope.line_to_byte(5);
        let dirty =
            compute_selection_reveal_dirty_lines(&rope, Some(&decorations), &[inside], &[outside]);
        assert_eq!(dirty, vec![0, 1, 2, 3, 5]);
    }

    #[test]
    fn caret_moving_inside_same_pipe_table_keeps_dirty_set_local() {
        let source = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nafter\n";
        let rope = Rope::from_str(source);
        let decorations = Decorations::compute(source, 1).unwrap();
        let old_inside = rope.line_to_byte(2) + 2;
        let new_inside = rope.line_to_byte(3) + 2;
        let dirty = compute_selection_reveal_dirty_lines(
            &rope,
            Some(&decorations),
            &[old_inside],
            &[new_inside],
        );
        assert_eq!(dirty, vec![2, 3]);
    }
}
