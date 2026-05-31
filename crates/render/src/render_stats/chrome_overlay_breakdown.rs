//! [`RendererChromeOverlayBreakdown`] — chrome-overlay sub-stage
//! durations measured by the renderer during one
//! `draw_buffer_no_present` call. UI stamps this into
//! [`crate::RenderStats`] for the chrome-overlay split fields on
//! `event:renderer_draw_stages`.
//!
//! Lifted out of the parent `render_stats.rs` so that file stays
//! under the 600-line conventions cap.

/// Chrome-overlay sub-stage durations measured by the renderer during
/// one `draw_buffer_no_present` call. All values in microseconds.
///
/// `outline_sidebar_us` and `scrollbar_us` are populated by mirroring
/// the post-body sub-stage timings (`RendererPostBodyStages.outline_us`
/// / `RendererPostBodyStages.scrollbar_us`); the dedicated cells let
/// trace consumers read the chrome-overlay split without joining two
/// event lines.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererChromeOverlayBreakdown {
    /// Line-number gutter and fold-triangle painters.
    pub line_numbers_us: u64,
    /// Indent guides and whitespace markers.
    pub indent_guides_us: u64,
    /// Current-line highlight and trailing-whitespace highlight.
    pub selection_bars_us: u64,
    /// Search-strip tick marks.
    pub search_ticks_us: u64,
    /// Fenced-code-block panels and blockquote bars.
    pub block_backgrounds_us: u64,
    /// Horizontal-rule rules.
    pub horizontal_rules_us: u64,
    /// Fenced-block copy-button hover affordance.
    pub code_copy_button_us: u64,
    /// Scaled-text minimap.
    pub minimap_us: u64,
    /// Outline sidebar paint.
    pub outline_sidebar_us: u64,
    /// Editor scrollbar paint.
    pub scrollbar_us: u64,
    /// Catch-all bucket for chrome work not named above (pane chrome,
    /// status bar, brushes setup, spectator bodies, focus-dim, spell,
    /// motion overlays, modal overlays, HUDs, scroll-tick placeholder
    /// strip, caret fallback, table-chrome plan prep).
    pub decoration_us: u64,
}

impl RendererChromeOverlayBreakdown {
    /// Sum every bucket. Reported as `chrome_overlay_sum_us` and
    /// validated against `chrome_overlay_us` (P0.5 5 % rule).
    #[must_use]
    pub fn sum_us(self) -> u64 {
        self.line_numbers_us
            .saturating_add(self.indent_guides_us)
            .saturating_add(self.selection_bars_us)
            .saturating_add(self.search_ticks_us)
            .saturating_add(self.block_backgrounds_us)
            .saturating_add(self.horizontal_rules_us)
            .saturating_add(self.code_copy_button_us)
            .saturating_add(self.minimap_us)
            .saturating_add(self.outline_sidebar_us)
            .saturating_add(self.scrollbar_us)
            .saturating_add(self.decoration_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_overlay_breakdown_sum_matches_each_field_total() {
        let breakdown = RendererChromeOverlayBreakdown {
            line_numbers_us: 10,
            indent_guides_us: 20,
            selection_bars_us: 30,
            search_ticks_us: 40,
            block_backgrounds_us: 50,
            horizontal_rules_us: 60,
            code_copy_button_us: 70,
            minimap_us: 80,
            outline_sidebar_us: 90,
            scrollbar_us: 100,
            decoration_us: 110,
        };
        assert_eq!(breakdown.sum_us(), 660);
    }
}
