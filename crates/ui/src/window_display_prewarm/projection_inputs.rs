//! Shared projection inputs derived from the live rope, decorations,
//! and selections.
//!
//! Used by both `on_paint` and the prewarm pipeline so they agree on
//! exactly what a `FrameDisplay` would project for a given snapshot.
//! Any divergence drives soft-wrap rows out of sync with the painted
//! pixels.
//!
//! All methods here read [`crate::Window`] state on the UI thread.

use continuity_buffer::BufferId;
use continuity_decorate::{BlockKind, Decorations};
use continuity_display_map::FoldRange;
use continuity_text::Selection;

use crate::window::Window;

/// Right-edge wrap safety gutter as a fraction of the (zoomed) font size.
/// Keeps glyph ink overhang and residual per-grapheme measurement rounding
/// from poking past the visible text column. Below one character wide, so
/// it does not visibly narrow the column. The paint geometry's right edge
/// is unchanged — this only makes soft-wrap fire a hair earlier.
const WRAP_SAFETY_MARGIN_EM: f32 = 0.25;

/// Projection geometry inputs shared by paint and prewarm.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DisplayProjectionMetrics {
    /// Soft-wrap width in DIPs; `0` disables wrapping.
    pub wrap_width_dip: u32,
    /// Monospace character advance approximation.
    pub char_width_dip: f32,
}

impl Window {
    /// Current projection metrics shared by `on_paint` and prewarm.
    #[must_use]
    pub(crate) fn display_projection_metrics(
        &self,
        search_minimap_active: bool,
        source_line_count: usize,
    ) -> DisplayProjectionMetrics {
        let scaled_font_size = self.scaled_font_size();
        let wrap_width_dip = if self.view.soft_wrap {
            let distraction_free = self.view_options.pane_modes.distraction_free;
            let distraction_free_max_width_dip = if distraction_free {
                self.view_options.pane_modes.distraction_free_max_width as f32
                    * scaled_font_size
                    * 0.55
            } else {
                0.0
            };
            let text_width = continuity_render::resolve_body_text_width_for_line_count_dip(
                self.view.viewport_width_dip,
                scaled_font_size,
                source_line_count,
                self.view_options.line_numbers,
                self.view_options.minimap,
                search_minimap_active,
                self.view_options.show_outline_sidebar,
                self.view_options.outline_sidebar_width_dip,
                distraction_free,
                distraction_free_max_width_dip,
            );
            (text_width - scaled_font_size * WRAP_SAFETY_MARGIN_EM)
                .round()
                .max(0.0) as u32
        } else {
            0
        };
        DisplayProjectionMetrics {
            wrap_width_dip,
            char_width_dip: (scaled_font_size * 0.55).max(1.0),
        }
    }

    /// `true` when the search-active minimap strip is reserving a column
    /// on the body's right edge — driven by the find-bar having at least
    /// one match. Used by hit-test / caret-anchor paths so their wrap
    /// projection matches what `on_paint` actually drew.
    #[must_use]
    pub(crate) fn current_search_minimap_active(&self) -> bool {
        self.overlays
            .find_bar()
            .is_some_and(|fb| !fb.matches.is_empty())
    }

    /// Current per-frame projection metrics keyed off the live search
    /// state. The single source of truth callers outside `on_paint`
    /// should use when building a `FrameDisplay` for hit-test or
    /// caret-anchor — any divergence drives soft-wrap rows out of sync
    /// with the painted pixels.
    #[must_use]
    pub(crate) fn current_display_projection_metrics(&self) -> DisplayProjectionMetrics {
        let source_line_count = self
            .editor
            .snapshot(self.buffer_id)
            .map(|snapshot| snapshot.rope_snapshot().rope().len_lines())
            .unwrap_or(1);
        self.display_projection_metrics(self.current_search_minimap_active(), source_line_count)
    }

    /// Convert selection heads into absolute caret bytes for display-map
    /// reveal decisions.
    #[must_use]
    pub(crate) fn caret_bytes_for_projection(
        rope: &ropey::Rope,
        selections: &[Selection],
    ) -> Vec<usize> {
        selections
            .iter()
            .map(|selection| {
                let line = selection.head.line as usize;
                let line_start = if line < rope.len_lines() {
                    rope.line_to_byte(line)
                } else {
                    rope.len_bytes()
                };
                line_start + selection.head.byte_in_line as usize
            })
            .collect()
    }

    /// Compute fold ranges exactly as the paint path needs them for a
    /// given rope/decorations/caret tuple.
    #[must_use]
    pub(crate) fn display_projection_folds(
        &self,
        rope: &ropey::Rope,
        heading_lines: &[(u32, u8)],
        caret_bytes: &[usize],
    ) -> Vec<FoldRange> {
        let mut fold_byte_ranges = continuity_core::compute_indent_fold_byte_ranges(
            rope,
            &self.view_options.pane_modes.folded_lines,
        );
        if !heading_lines.is_empty() {
            fold_byte_ranges.extend(continuity_core::compute_heading_fold_byte_ranges(
                rope,
                heading_lines,
                &self.view_options.pane_modes.folded_lines,
            ));
        }
        // The pipe-table alignment row is *not* folded — folding would
        // collapse its source-line slot to zero display lines, which
        // makes the row vanish entirely and slides the body up against
        // the header. Hiding the row's bytes + painting styled chrome
        // through the slot is handled by
        // `continuity_display_map::table_hide_provider` and
        // `continuity_render::table_paint::paint_alignment_row_dividers`;
        // those keep one display line for the row and the source ↔
        // display mapping stays 1:1.
        let _ = caret_bytes;
        fold_byte_ranges.sort_unstable_by_key(|range| range.start_byte);
        let mut coalesced: Vec<continuity_core::IndentFoldByteRange> = Vec::new();
        for range in fold_byte_ranges {
            if let Some(previous) = coalesced.last_mut() {
                if range.start_byte <= previous.end_byte {
                    previous.end_byte = previous.end_byte.max(range.end_byte);
                    continue;
                }
            }
            coalesced.push(range);
        }
        coalesced
            .iter()
            .filter_map(|range| {
                FoldRange::new(
                    continuity_display_map::SourceByte::from_usize(range.start_byte),
                    continuity_display_map::SourceByte::from_usize(range.end_byte),
                )
            })
            .collect()
    }

    /// Heading fold input derived from a current decoration snapshot.
    #[must_use]
    /// Extract `(line, level)` for every heading in `decorations`.
    ///
    /// Uses [`ropey::Rope::byte_to_line`] (O(log N) per heading) directly
    /// against the live rope instead of materialising `rope.to_string()`
    /// and scanning bytes linearly per heading. On the canonical 6 k-line
    /// markdown buffer this drops the per-call cost from ~44 ms p95
    /// (rope-to-string + 50 × `bytes.iter().filter(== b'\n').count()`)
    /// to single-digit microseconds, removing the dominant remaining
    /// component of `event:edit_apply` p95 after the ε.5e+coalesce fix.
    pub(crate) fn heading_lines_for_projection(
        rope: &ropey::Rope,
        decorations: Option<&Decorations>,
    ) -> Vec<(u32, u8)> {
        let Some(decorations) = decorations else {
            return Vec::new();
        };
        let len_bytes = rope.len_bytes();
        let mut out = Vec::with_capacity(decorations.blocks.len());
        for span in &decorations.blocks {
            let level = match span.kind {
                BlockKind::Heading { level } | BlockKind::SetextHeading { level } => level,
                _ => continue,
            };
            // Clamp first — a stale decoration captured at an earlier
            // rope revision can carry byte offsets that overshoot the
            // current rope's length during a typing burst (the
            // decoration worker hasn't re-parsed yet). Clamping keeps
            // the line lookup well-defined and the worst case is a
            // line number that's one or two off — paint then rebuilds
            // the affected display rows on the next decoration update.
            let byte = span.start_byte.min(len_bytes);
            let line = rope.byte_to_line(byte) as u32;
            out.push((line, level));
        }
        out
    }

    /// Cached counterpart of [`Self::heading_lines_for_projection`].
    /// Keyed by `(buffer, decoration_revision, rope.len_lines())`
    /// — see [`crate::window_heading_lines_cache::HeadingLinesCacheEntry`] for the cache
    /// design rationale. Returns a fresh `Vec<(line, level)>` for
    /// the caller to own; the cache stores its own clone.
    ///
    /// The `rope_revision` parameter is retained in the signature for
    /// call-site symmetry with other projection inputs, but does not
    /// participate in cache key matching.
    #[must_use]
    pub(crate) fn cached_heading_lines_for_projection(
        &self,
        buffer_id: BufferId,
        rope: &ropey::Rope,
        _rope_revision: u64,
        decorations: Option<&Decorations>,
    ) -> Vec<(u32, u8)> {
        let decoration_revision = decorations.map(|d| d.revision);
        let rope_line_count = rope.len_lines();
        {
            let borrowed = self.heading_lines_cache.borrow();
            if let Some(entry) = borrowed.as_ref() {
                if entry.buffer == buffer_id
                    && entry.rope_line_count == rope_line_count
                    && entry.decoration_revision == decoration_revision
                {
                    return entry.headings.clone();
                }
            }
        }
        let headings = Self::heading_lines_for_projection(rope, decorations);
        *self.heading_lines_cache.borrow_mut() =
            Some(crate::window_heading_lines_cache::HeadingLinesCacheEntry {
                buffer: buffer_id,
                rope_line_count,
                decoration_revision,
                headings: headings.clone(),
            });
        headings
    }
}
