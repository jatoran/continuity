//! `FrameDisplay` build wrappers used by both paint and prewarm.
//!
//! Each wrapper picks the DirectWrite-backed glyph measurer when the
//! window has already finished render setup and falls back to the
//! scalar `FixedCharWidth` approximation otherwise. Centralised here
//! so paint and the prewarm stage agree on exactly how a frame is
//! projected.
//!
//! Runs on the [`crate::Window`]-owning UI thread.

use continuity_buffer::BufferId;
use continuity_decorate::Decorations;
use continuity_display_map::{FoldRange, FoldSignature, ImageRowReservation, WalkerCallReason};
use continuity_render::{DirectWriteWidthMeasure, FrameDisplay};
use std::sync::Arc;

use crate::window::Window;
use crate::window_row_index_cache::{compute_decoration_row_shape_signature, RowIndexKey};

#[derive(Copy, Clone, Eq, PartialEq)]
enum RowIndexLookupSource {
    Exact,
    Compatible,
}

fn row_index_with_current_decoration_stamp(
    row_index: Arc<continuity_display_map::DisplayRowIndex>,
    revision: u64,
    decorations: Option<&Decorations>,
) -> Arc<continuity_display_map::DisplayRowIndex> {
    let current_decoration_revision = decorations.map_or(revision, |d| d.revision);
    if row_index.stamps().decoration_revision == current_decoration_revision {
        return row_index;
    }
    let mut advanced = (*row_index).clone();
    let mut stamps = *advanced.stamps();
    stamps.decoration_revision = current_decoration_revision;
    advanced.set_stamps(stamps);
    Arc::new(advanced)
}

fn row_index_validation_miss_reason(
    row_index: &continuity_display_map::DisplayRowIndex,
    rope: &ropey::Rope,
    revision: u64,
    decorations: Option<&Decorations>,
    folds: &[FoldRange],
    wrap_width_dip: u32,
) -> Option<&'static str> {
    let stamps = row_index.stamps();
    let expected_decoration_revision = decorations.map_or(revision, |d| d.revision);
    if row_index.source_line_count() != rope.len_lines() as u32 {
        Some("source_line_count")
    } else if stamps.rope_revision != revision {
        Some("rope_revision")
    } else if stamps.decoration_revision != expected_decoration_revision {
        Some("decoration_revision")
    } else if stamps.wrap_width_dip != wrap_width_dip {
        Some("wrap_width_dip")
    } else if stamps.fold_signature != FoldSignature::compute(folds) {
        Some("fold_signature")
    } else {
        None
    }
}

fn row_index_key_for(
    buffer_id: Option<BufferId>,
    revision: u64,
    decorations: Option<&Decorations>,
    folds: &[FoldRange],
    image_reservations: &[ImageRowReservation],
    wrap_width_dip: u32,
    font_state: continuity_layout::FontStateId,
) -> Option<RowIndexKey> {
    // Cache the row index only when reservations are empty —
    // image reservations inject phantom rows into the index, so
    // a frame built with one reservation set isn't valid for
    // another. The same constraint already applies to the
    // focused / spectator frame caches.
    buffer_id
        .filter(|_| image_reservations.is_empty())
        .map(|bid| RowIndexKey {
            document: bid.as_uuid().as_u128(),
            rope_revision: revision,
            decoration_revision: decorations.map(|d| d.revision),
            wrap_width_dip,
            font_state,
            fold_signature: FoldSignature::compute(folds),
            decoration_row_shape_signature: compute_decoration_row_shape_signature(decorations),
        })
}

impl Window {
    /// Build a frame display using DirectWrite glyph metrics when the
    /// window already has a text format. Falls back to the scalar
    /// approximation before the first render setup has completed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_frame_display_with_options(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
    ) -> FrameDisplay {
        if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
            );
            return FrameDisplay::build_with_options_measured(
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                wrap_width_dip,
                &mut measure,
            );
        }
        FrameDisplay::build_with_options(
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            wrap_width_dip,
            fallback_char_width_dip,
        )
    }

    /// `true` when the next cached viewport build can reuse a strict
    /// or compatible row index. This does not mutate LRU order; it is
    /// only a trace probe for callers that immediately invoke
    /// [`Self::build_frame_display_viewport_cached`].
    pub(crate) fn has_cached_row_index_for_frame_display_viewport(
        &self,
        buffer_id: Option<BufferId>,
        revision: u64,
        decorations: Option<&Decorations>,
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
    ) -> bool {
        row_index_key_for(
            buffer_id,
            revision,
            decorations,
            folds,
            image_reservations,
            wrap_width_dip,
            self.font_state,
        )
        .is_some_and(|key| {
            self.row_index_cache
                .borrow()
                .contains_exact_or_compatible(&key)
        })
    }

    /// Cross-pane row-index cached viewport build. When `buffer_id`
    /// is `Some(_)` and the cross-pane row-index cache has an entry
    /// for `(buffer_id, rope_revision, decoration_revision,
    /// wrap_width_dip, font_state, fold_signature)`, the
    /// `DisplayRowIndex` is reused and the cheap row-count walker is
    /// skipped — turning a cold ~400 ms viewport build on a 9 k-line
    /// markdown buffer into ~5-10 ms of spec materialization for the
    /// visible viewport.
    ///
    /// Pass `buffer_id: None` to bypass the cache (used by call
    /// sites that don't know their buffer id or that don't want to
    /// pollute the cache).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_frame_display_viewport_cached(
        &self,
        buffer_id: Option<BufferId>,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
        walker_reason: WalkerCallReason,
    ) -> FrameDisplay {
        let cache_key = row_index_key_for(
            buffer_id,
            revision,
            decorations,
            folds,
            image_reservations,
            wrap_width_dip,
            self.font_state,
        );
        let cached_index = cache_key.as_ref().and_then(|k| {
            let mut cache = self.row_index_cache.borrow_mut();
            if let Some(row_index) = cache.get(k) {
                Some((row_index, RowIndexLookupSource::Exact))
            } else {
                cache
                    .get_compatible(k)
                    .map(|row_index| (row_index, RowIndexLookupSource::Compatible))
            }
        });

        let fd = if let Some((row_index, lookup_source)) = cached_index {
            if crate::paint_trace::is_trace_enabled() {
                let detail = if lookup_source == RowIndexLookupSource::Compatible {
                    format!("hit=compat document={:?}", buffer_id)
                } else {
                    format!("hit document={:?}", buffer_id)
                };
                crate::paint_trace::log_event("row_index_cache", &detail);
            }
            let row_index = if lookup_source == RowIndexLookupSource::Compatible {
                row_index_with_current_decoration_stamp(row_index, revision, decorations)
            } else {
                row_index
            };
            if let Some(reason) = row_index_validation_miss_reason(
                &row_index,
                rope,
                revision,
                decorations,
                folds,
                wrap_width_dip,
            ) {
                if crate::paint_trace::is_trace_enabled() {
                    crate::paint_trace::log_event(
                        "row_index_cache",
                        &format!("stale={reason} document={:?}", buffer_id),
                    );
                }
                self.cold_build_with_split_trace(
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    fallback_char_width_dip,
                    visible_rows,
                    overscan,
                    walker_reason,
                )
            } else if let Some(format) = self.text_format.as_ref() {
                let mut measure = DirectWriteWidthMeasure::new(
                    self.dwrite.raw(),
                    format,
                    self.scaled_font_size(),
                    continuity_render::DEFAULT_HEADING_SCALE,
                    fallback_char_width_dip,
                );
                FrameDisplay::build_viewport_with_row_index_measured(
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    self.markdown_render_toggles(),
                    wrap_width_dip,
                    &mut measure,
                    visible_rows,
                    overscan,
                    row_index,
                )
            } else {
                let mut measure = continuity_display_map::wrap::FixedCharWidth::new(
                    fallback_char_width_dip.max(1.0),
                );
                FrameDisplay::build_viewport_with_row_index_measured(
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    self.markdown_render_toggles(),
                    wrap_width_dip,
                    &mut measure,
                    visible_rows,
                    overscan,
                    row_index,
                )
            }
        } else {
            // Cross-revision splice fast-path. Only attempt when a
            // same-geometry entry exists at a different `rope_revision`
            // and the delta history covers the gap; the splice can't
            // bridge wrap-width / font / fold drift on its own. When
            // the splice succeeds we skip the `action=miss` trace
            // entirely so the standing miss counter only counts cases
            // that fell through to the cold walker.
            let splice_started = std::time::Instant::now();
            let splice_outcome = match (cache_key.as_ref(), buffer_id) {
                (Some(k), Some(bid)) => self.try_splice_row_index_forward(
                    k,
                    bid,
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    fallback_char_width_dip,
                ),
                _ => None,
            };
            if let Some(spliced) = splice_outcome {
                if crate::paint_trace::is_trace_enabled() {
                    let elapsed_us = splice_started.elapsed().as_micros() as u64;
                    crate::paint_trace::log_event(
                        "row_index_splice",
                        &format!(
                            "path=splice document={:?} dirty_lines={} shift_bytes={} used_row_splice={} prev_rev={} cur_rev={} elapsed_us={}",
                            buffer_id,
                            spliced.stats.dirty_lines,
                            spliced.stats.shift_bytes,
                            spliced.stats.used_row_splice,
                            spliced.prev_rope_revision,
                            revision,
                            elapsed_us,
                        ),
                    );
                }
                if let Some(format) = self.text_format.as_ref() {
                    let mut measure = DirectWriteWidthMeasure::new(
                        self.dwrite.raw(),
                        format,
                        self.scaled_font_size(),
                        continuity_render::DEFAULT_HEADING_SCALE,
                        fallback_char_width_dip,
                    );
                    FrameDisplay::build_viewport_with_row_index_measured(
                        rope,
                        revision,
                        decorations,
                        caret_bytes,
                        folds,
                        image_reservations,
                        self.markdown_render_toggles(),
                        wrap_width_dip,
                        &mut measure,
                        visible_rows,
                        overscan,
                        spliced.row_index,
                    )
                } else {
                    let mut measure = continuity_display_map::wrap::FixedCharWidth::new(
                        fallback_char_width_dip.max(1.0),
                    );
                    FrameDisplay::build_viewport_with_row_index_measured(
                        rope,
                        revision,
                        decorations,
                        caret_bytes,
                        folds,
                        image_reservations,
                        self.markdown_render_toggles(),
                        wrap_width_dip,
                        &mut measure,
                        visible_rows,
                        overscan,
                        spliced.row_index,
                    )
                }
            } else {
                if crate::paint_trace::is_trace_enabled() && cache_key.is_some() {
                    let miss_reason = cache_key
                        .as_ref()
                        .map(|k| self.row_index_cache.borrow().closest_match_diff(k))
                        .unwrap_or("no_entry");
                    crate::paint_trace::log_event(
                        "row_index_cache",
                        &format!("action=miss document={:?} reason={miss_reason}", buffer_id),
                    );
                }
                // Split cold build into the row-count walker phase and
                // the viewport materialization phase so paint traces
                // can attribute the per-paint cost. The walker phase
                // publishes `paint:row_count_walker` (span dur) +
                // `paint:row_count_walker_stats` (per-decision-path
                // counters); the materialize phase publishes
                // `paint:viewport_materialize`. Combining the two
                // reproduces the prior `paint:frame_display:cold_build`
                // cost.
                self.cold_build_with_split_trace(
                    rope,
                    revision,
                    decorations,
                    caret_bytes,
                    folds,
                    image_reservations,
                    wrap_width_dip,
                    fallback_char_width_dip,
                    visible_rows,
                    overscan,
                    walker_reason,
                )
            }
        };

        // Seed the cache with the freshly-built row index so the
        // next paint of this buffer at the same geometry — in any
        // pane / tab / layout — reuses it.
        if let Some(key) = cache_key {
            self.row_index_cache
                .borrow_mut()
                .insert(key, fd.row_index_arc());
        }
        fd
    }

    /// ε.3 — dirty rebuild against a previous painted frame. Reuses
    /// `prev`'s realized `DisplayLineSpec`s for clean source lines and
    /// materializes fresh specs only for `dirty` source lines.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rebuild_frame_display_dirty(
        &self,
        prev: &FrameDisplay,
        dirty: &[u32],
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&Decorations>,
        caret_bytes: &[usize],
        folds: &[FoldRange],
        image_reservations: &[ImageRowReservation],
        suppressed_table_blocks: &[std::ops::Range<usize>],
        wrap_width_dip: u32,
        fallback_char_width_dip: f32,
        visible_rows: std::ops::Range<u32>,
        overscan: u32,
    ) -> FrameDisplay {
        if let Some(format) = self.text_format.as_ref() {
            let mut measure = DirectWriteWidthMeasure::new(
                self.dwrite.raw(),
                format,
                self.scaled_font_size(),
                continuity_render::DEFAULT_HEADING_SCALE,
                fallback_char_width_dip,
            );
            return FrameDisplay::rebuild_dirty_measured(
                prev,
                dirty,
                rope,
                revision,
                decorations,
                caret_bytes,
                folds,
                image_reservations,
                suppressed_table_blocks,
                self.markdown_render_toggles(),
                wrap_width_dip,
                &mut measure,
                visible_rows,
                overscan,
            );
        }
        let mut measure =
            continuity_display_map::wrap::FixedCharWidth::new(fallback_char_width_dip.max(1.0));
        FrameDisplay::rebuild_dirty_measured(
            prev,
            dirty,
            rope,
            revision,
            decorations,
            caret_bytes,
            folds,
            image_reservations,
            suppressed_table_blocks,
            self.markdown_render_toggles(),
            wrap_width_dip,
            &mut measure,
            visible_rows,
            overscan,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_decorate::Decorations;
    use continuity_display_map::{DisplayRowIndex, IndexStamps};
    use ropey::Rope;

    fn row_index(source_lines: usize, revision: u64) -> DisplayRowIndex {
        DisplayRowIndex::from_row_counts(
            vec![1u16; source_lines],
            IndexStamps {
                rope_revision: revision,
                decoration_revision: revision,
                wrap_width_dip: 0,
                font_state: 0,
                fold_signature: 0,
            },
        )
    }

    #[test]
    fn row_index_validation_rejects_source_line_count_drift() {
        let rope = Rope::from_str("a\nb\nc");
        let index = row_index(2, 7);

        assert_eq!(
            row_index_validation_miss_reason(&index, &rope, 7, None, &[], 0),
            Some("source_line_count")
        );
    }

    #[test]
    fn row_index_validation_rejects_rope_revision_drift() {
        let rope = Rope::from_str("a\nb\nc");
        let index = row_index(rope.len_lines(), 6);

        assert_eq!(
            row_index_validation_miss_reason(&index, &rope, 7, None, &[], 0),
            Some("rope_revision")
        );
    }

    #[test]
    fn compatible_row_index_stamp_updates_only_decoration_revision() {
        let index = Arc::new(row_index(1, 7));
        let decorations = Decorations::empty(9);

        let advanced = row_index_with_current_decoration_stamp(index, 7, Some(&decorations));

        assert_eq!(advanced.stamps().rope_revision, 7);
        assert_eq!(advanced.stamps().decoration_revision, 9);
    }
}
