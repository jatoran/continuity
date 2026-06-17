//! Caret display-row builders for the screen-y anchor and caret reveal.
//!
//! These [`Window`] methods materialize a `FrameDisplay` whose realized
//! window covers the caret so its display row can be measured EXACTLY,
//! rather than estimated from the document-average wrap factor. Split out
//! of `window_caret_anchor.rs` to keep that file under the 600-line cap;
//! the resolution types and the cheap-cache lookup
//! ([`Window::resolve_caret_display_line`]) stay in the parent.
//!
//! Thread ownership: UI-thread-only, like the rest of caret anchoring.

use continuity_text::Position;

use super::{compute_caret_display_line_from_frame, CaretDisplayLine};
use crate::window::Window;

impl Window {
    /// Viewport-bounded frame-display build used by caret anchoring.
    /// Pulled out so the path with and without a cached frame share
    /// the same viewport-row math and overscan policy.
    pub(super) fn build_caret_anchor_viewport_frame_display(
        &self,
        rope: &ropey::Rope,
        revision: u64,
        decorations: Option<&continuity_decorate::Decorations>,
        caret_bytes: &[usize],
        wrap_width_dip: u32,
        char_width_dip: f32,
    ) -> continuity_render::FrameDisplay {
        let visible_rows = crate::window_paint::visible_display_row_range(
            self.view.scroll_y_dip,
            self.view.viewport_height_dip,
            self.effective_line_height(),
        );
        self.build_frame_display_viewport_cached(
            Some(self.buffer_id),
            rope,
            revision,
            decorations,
            caret_bytes,
            &[],
            &[],
            wrap_width_dip,
            char_width_dip,
            visible_rows,
            crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
            continuity_display_map::WalkerCallReason::ViewportRealize,
        )
    }

    /// Resolve the caret's display row only when it can be measured
    /// EXACTLY — never an estimate. Returns `Some` for a `RealizedSpec`
    /// (the caret's own row was materialized) or a `RowIndexOnly` lookup
    /// against a fully-built (non-partial) index; both are true display
    /// positions. Returns `None` when the only thing available is a
    /// density-scaled / source-floor estimate over a partial index.
    ///
    /// On the first miss against the cheap caches it forces one
    /// viewport-bounded build anchored at the caret region, so the caret's
    /// own row gets realized (O(visible+overscan)), then re-resolves. This
    /// is the "build a fresh frame for the caret region to get the TRUE
    /// display row" path: a wrapped buffer's average-wrap estimate
    /// over/undershoots per-line, so the reveal must measure rather than
    /// guess.
    pub(crate) fn try_resolve_caret_display_row_exact(
        &self,
        caret: Position,
    ) -> Option<CaretDisplayLine> {
        if let Some(line) = self.resolve_caret_display_line(caret) {
            if !line.needs_scaled_reveal_estimate() {
                return Some(line);
            }
        }
        // The cheap path could only produce an estimate. Force a build
        // anchored at the caret region so the caret's own row is
        // materialized, then trust only a measured result.
        let line = self.build_and_resolve_caret_region(caret)?;
        (!line.needs_scaled_reveal_estimate()).then_some(line)
    }

    /// Build a `FrameDisplay` whose realized window covers the caret's
    /// own display row, then resolve the caret's display line against it.
    /// Used as the measured fallback by
    /// [`Self::try_resolve_caret_display_row_exact`] when the cached
    /// projections only yield a partial-index estimate.
    ///
    /// Prefers the targeted row-index refresh: it patches the previous
    /// whole-document index for the caret's source lines and materializes
    /// the caret line's display-row range, so the caret row lands as a
    /// `RealizedSpec` against an index whose prefix sum to the caret line
    /// is correct (not the partial placeholder under-count). When no
    /// previous frame exists to patch, falls back to a viewport-bounded
    /// build seeded at the caret's source-line index.
    fn build_and_resolve_caret_region(&self, caret: Position) -> Option<CaretDisplayLine> {
        let snap = self.editor.snapshot(self.buffer_id)?;
        let rope = snap.rope_snapshot().rope();
        let revision = snap.rope_snapshot().revision().0;
        let decorations = self
            .decoration_cache
            .get(self.buffer_id.as_uuid().as_u128());
        let line = caret.line as usize;
        let line_start = if line < rope.len_lines() {
            rope.line_to_byte(line)
        } else {
            rope.len_bytes()
        };
        let caret_byte = line_start + caret.byte_in_line as usize;
        let caret_bytes = [caret_byte];
        let metrics =
            self.display_projection_metrics(self.current_search_minimap_active(), rope.len_lines());
        let query = crate::display_prewarm_cache::PrewarmQuery::new(
            self.buffer_id,
            revision,
            decorations.map(|decorations| decorations.revision),
            &caret_bytes,
            &[],
            metrics.wrap_width_dip,
            self.font_state,
        );
        let fd = if let Some(fd) = self.build_caret_anchor_targeted_frame_display(
            &query,
            rope,
            revision,
            decorations,
            &caret_bytes,
            line,
            metrics.wrap_width_dip,
            metrics.char_width_dip,
        ) {
            fd
        } else {
            // No previous frame to patch — seed a viewport-bounded build
            // at the caret's source-line index. On a 1:1 buffer this
            // brackets the caret row exactly; on a wrapped buffer the
            // walk realizes the source line's rows so the caret byte
            // resolves to a `RealizedSpec`.
            let total_source_lines = rope.len_lines().max(1) as u32;
            let seed_row = (line as u32).min(total_source_lines.saturating_sub(1));
            let line_height = self.effective_line_height();
            let viewport_rows = (self.view.viewport_height_dip / line_height)
                .ceil()
                .max(1.0) as u32;
            let visible_rows = seed_row..seed_row.saturating_add(viewport_rows);
            self.build_frame_display_viewport_cached(
                Some(self.buffer_id),
                rope,
                revision,
                decorations,
                &caret_bytes,
                &[],
                &[],
                metrics.wrap_width_dip,
                metrics.char_width_dip,
                visible_rows,
                crate::window_paint::VIEWPORT_OVERSCAN_ROWS,
                continuity_display_map::WalkerCallReason::ViewportRealize,
            )
        };
        compute_caret_display_line_from_frame(&fd, line, caret.byte_in_line as usize)
    }
}
