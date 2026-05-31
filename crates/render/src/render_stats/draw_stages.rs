//! [`RendererDrawStages`] — aggregate renderer draw-stage durations
//! for one paint, with the chrome-overlay sub-stage split that
//! `event:renderer_draw_stages` consumes. Lifted out of the parent
//! `render_stats.rs` so that file stays under the 600-line
//! conventions cap.

/// Aggregate renderer draw-stage durations for one paint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererDrawStages {
    /// Body text layout draw calls.
    pub body_glyphs_us: u64,
    /// Selection rectangles and text-line highlights.
    pub selection_overlay_us: u64,
    /// Markdown block backgrounds, rules, and code/table fills.
    pub decoration_overlay_us: u64,
    /// Editor chrome: guides, ruler, line numbers, panes, bars
    /// (everything except the per-table chrome split below).
    pub chrome_overlay_us: u64,
    /// Per-table chrome record + replay time (P14.1). Lives in its
    /// own bucket so trace consumers can see the table-only cost
    /// directly instead of inferring it from a chrome-overlay delta.
    pub chrome_overlay_table_us: u64,
    /// Inline-image paint pass.
    pub inline_images_us: u64,
    /// Sub-stages of `chrome_overlay_us`. The named buckets below
    /// sum into `chrome_overlay_sum_us`; trace consumers check that
    /// the sum lands within 5 % of `chrome_overlay_us` (same
    /// accounting contract P0.5 applied to draw stages).
    pub chrome_overlay_line_numbers_us: u64,
    /// Indent-guide rules and whitespace markers.
    pub chrome_overlay_indent_guides_us: u64,
    /// Current-line highlight and trailing-whitespace highlight.
    pub chrome_overlay_selection_bars_us: u64,
    /// Search-strip tick marks.
    pub chrome_overlay_search_ticks_us: u64,
    /// Fenced code panels and blockquote bars (`paint_block_backgrounds`).
    pub chrome_overlay_block_backgrounds_us: u64,
    /// Horizontal-rule rules (`paint_horizontal_rules`).
    pub chrome_overlay_horizontal_rules_us: u64,
    /// Fenced-block copy-button hover affordance.
    pub chrome_overlay_code_copy_button_us: u64,
    /// Scaled-text minimap.
    pub chrome_overlay_minimap_us: u64,
    /// Outline sidebar paint.
    pub chrome_overlay_outline_sidebar_us: u64,
    /// Editor scrollbar paint.
    pub chrome_overlay_scrollbar_us: u64,
    /// Everything else inside the chrome overlay envelope (pane
    /// chrome, status bar, spectator bodies, brushes setup, focus-dim,
    /// spell squiggles, motion overlays, modal overlays, HUDs,
    /// scroll-tick placeholder strip, caret fallback).
    pub chrome_overlay_decoration_us: u64,
}

impl RendererDrawStages {
    /// Sum every reported top-level bucket. The chrome-overlay
    /// sub-stages live inside `chrome_overlay_us` and are deliberately
    /// excluded from this total to avoid double-counting.
    #[must_use]
    pub fn total_us(self) -> u64 {
        self.body_glyphs_us
            .saturating_add(self.selection_overlay_us)
            .saturating_add(self.decoration_overlay_us)
            .saturating_add(self.chrome_overlay_us)
            .saturating_add(self.chrome_overlay_table_us)
            .saturating_add(self.inline_images_us)
    }

    /// Sum of the chrome-overlay sub-stages. Should land within 5 %
    /// of `chrome_overlay_us` — the accounting contract that lets
    /// trace consumers verify the breakdown is complete.
    #[must_use]
    pub fn chrome_overlay_sum_us(self) -> u64 {
        self.chrome_overlay_line_numbers_us
            .saturating_add(self.chrome_overlay_indent_guides_us)
            .saturating_add(self.chrome_overlay_selection_bars_us)
            .saturating_add(self.chrome_overlay_search_ticks_us)
            .saturating_add(self.chrome_overlay_block_backgrounds_us)
            .saturating_add(self.chrome_overlay_horizontal_rules_us)
            .saturating_add(self.chrome_overlay_code_copy_button_us)
            .saturating_add(self.chrome_overlay_minimap_us)
            .saturating_add(self.chrome_overlay_outline_sidebar_us)
            .saturating_add(self.chrome_overlay_scrollbar_us)
            .saturating_add(self.chrome_overlay_decoration_us)
    }

    /// `true` when stage sums are within 5% of an enclosing measured draw.
    #[must_use]
    pub fn is_within_enclosing_draw_duration(self, enclosing_us: u64) -> bool {
        if enclosing_us == 0 {
            return self.total_us() == 0;
        }
        let total = self.total_us();
        let delta = total.abs_diff(enclosing_us);
        delta.saturating_mul(100) <= enclosing_us.saturating_mul(5)
    }

    /// `true` when the chrome-overlay sub-stage sum lands within 5 %
    /// of `chrome_overlay_us`. Returns `true` for the trivial case
    /// where the parent bucket is zero — nothing to break down.
    #[must_use]
    pub fn is_chrome_overlay_breakdown_within_five_percent(self) -> bool {
        let parent = self.chrome_overlay_us;
        if parent == 0 {
            return self.chrome_overlay_sum_us() == 0;
        }
        let sum = self.chrome_overlay_sum_us();
        let delta = sum.abs_diff(parent);
        delta.saturating_mul(100) <= parent.saturating_mul(5)
    }

    /// Format the TSV details column for `event:renderer_draw_stages`.
    #[must_use]
    pub fn trace_detail(self) -> String {
        format!(
            concat!(
                "body_glyphs_us={} selection_overlay_us={} ",
                "decoration_overlay_us={} chrome_overlay_us={} ",
                "chrome_overlay_table_us={} inline_images_us={} ",
                "chrome_overlay_line_numbers_us={} chrome_overlay_indent_guides_us={} ",
                "chrome_overlay_selection_bars_us={} chrome_overlay_search_ticks_us={} ",
                "chrome_overlay_block_backgrounds_us={} ",
                "chrome_overlay_horizontal_rules_us={} ",
                "chrome_overlay_code_copy_button_us={} chrome_overlay_minimap_us={} ",
                "chrome_overlay_outline_sidebar_us={} chrome_overlay_scrollbar_us={} ",
                "chrome_overlay_decoration_us={} chrome_overlay_sum_us={} ",
                "stage_sum_us={}"
            ),
            self.body_glyphs_us,
            self.selection_overlay_us,
            self.decoration_overlay_us,
            self.chrome_overlay_us,
            self.chrome_overlay_table_us,
            self.inline_images_us,
            self.chrome_overlay_line_numbers_us,
            self.chrome_overlay_indent_guides_us,
            self.chrome_overlay_selection_bars_us,
            self.chrome_overlay_search_ticks_us,
            self.chrome_overlay_block_backgrounds_us,
            self.chrome_overlay_horizontal_rules_us,
            self.chrome_overlay_code_copy_button_us,
            self.chrome_overlay_minimap_us,
            self.chrome_overlay_outline_sidebar_us,
            self.chrome_overlay_scrollbar_us,
            self.chrome_overlay_decoration_us,
            self.chrome_overlay_sum_us(),
            self.total_us(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_stats::chrome_overlay_breakdown::RendererChromeOverlayBreakdown;
    use crate::RenderStats;

    fn breakdown_summing_to(total: u64) -> RendererChromeOverlayBreakdown {
        let half = total / 2;
        let rem = total - half;
        RendererChromeOverlayBreakdown {
            block_backgrounds_us: half,
            decoration_us: rem,
            ..RendererChromeOverlayBreakdown::default()
        }
    }

    #[test]
    fn renderer_draw_stage_sum_accepts_five_percent_window() {
        let stages = RendererDrawStages {
            body_glyphs_us: 700,
            selection_overlay_us: 100,
            decoration_overlay_us: 50,
            chrome_overlay_us: 100,
            chrome_overlay_table_us: 0,
            inline_images_us: 0,
            ..RendererDrawStages::default()
        };
        assert!(stages.is_within_enclosing_draw_duration(1_000));
        assert!(!stages.is_within_enclosing_draw_duration(1_100));
    }

    #[test]
    fn table_chrome_us_contributes_to_draw_stage_total() {
        let stages = RendererDrawStages {
            body_glyphs_us: 200,
            chrome_overlay_us: 100,
            chrome_overlay_table_us: 700,
            ..RendererDrawStages::default()
        };
        assert_eq!(stages.total_us(), 1_000);
        assert!(stages.is_within_enclosing_draw_duration(1_000));
    }

    #[test]
    fn draw_stages_carries_chrome_overlay_breakdown_into_trace_detail() {
        let stats = RenderStats {
            body_paint_us: 1_000,
            post_body_paint_us: 5_000,
            chrome_overlay_breakdown: breakdown_summing_to(5_000),
            ..RenderStats::default()
        };
        let stages = stats.draw_stages();
        assert_eq!(stages.chrome_overlay_us, 5_000);
        assert_eq!(stages.chrome_overlay_sum_us(), 5_000);
        let detail = stages.trace_detail();
        for name in [
            "chrome_overlay_line_numbers_us=",
            "chrome_overlay_indent_guides_us=",
            "chrome_overlay_selection_bars_us=",
            "chrome_overlay_search_ticks_us=",
            "chrome_overlay_block_backgrounds_us=",
            "chrome_overlay_horizontal_rules_us=",
            "chrome_overlay_code_copy_button_us=",
            "chrome_overlay_minimap_us=",
            "chrome_overlay_outline_sidebar_us=",
            "chrome_overlay_scrollbar_us=",
            "chrome_overlay_decoration_us=",
            "chrome_overlay_sum_us=",
        ] {
            assert!(detail.contains(name), "missing {name} in {detail}");
        }
    }

    #[test]
    fn chrome_overlay_breakdown_within_five_percent_passes_exact_sum() {
        let stats = RenderStats {
            post_body_paint_us: 10_000,
            chrome_overlay_breakdown: breakdown_summing_to(10_000),
            ..RenderStats::default()
        };
        assert!(stats
            .draw_stages()
            .is_chrome_overlay_breakdown_within_five_percent());
    }

    #[test]
    fn chrome_overlay_breakdown_within_five_percent_rejects_large_drift() {
        // `draw_stages()` makes the parity hold by construction;
        // exercise the predicate by manually building a
        // `RendererDrawStages` whose `chrome_overlay_us` is larger
        // than the sum-of-sub-stages — the drift case a trace
        // consumer would see if the renderer's measurements grew
        // incomplete.
        let stages = RendererDrawStages {
            chrome_overlay_us: 10_000,
            chrome_overlay_block_backgrounds_us: 2_500,
            chrome_overlay_decoration_us: 2_500,
            ..RendererDrawStages::default()
        };
        assert!(!stages.is_chrome_overlay_breakdown_within_five_percent());
    }

    #[test]
    fn draw_stages_for_enclosing_folds_gap_into_decoration_catch_all() {
        let stats = RenderStats {
            body_paint_us: 1_000,
            post_body_paint_us: 5_000,
            chrome_overlay_breakdown: breakdown_summing_to(5_000),
            ..RenderStats::default()
        };
        let enclosing = 10_000;
        let stages = stats.draw_stages_for_enclosing(Some(enclosing));
        assert_eq!(stages.chrome_overlay_us, 5_000 + (enclosing - 6_000));
        assert!(stages.is_chrome_overlay_breakdown_within_five_percent());
        assert!(stages.is_within_enclosing_draw_duration(enclosing));
    }

    #[test]
    fn chrome_overlay_sub_stages_are_excluded_from_top_level_total() {
        let stages = RendererDrawStages {
            body_glyphs_us: 1_000,
            chrome_overlay_us: 5_000,
            chrome_overlay_block_backgrounds_us: 2_500,
            chrome_overlay_decoration_us: 2_500,
            ..RendererDrawStages::default()
        };
        assert_eq!(stages.total_us(), 6_000);
        assert_eq!(stages.chrome_overlay_sum_us(), 5_000);
    }
}
