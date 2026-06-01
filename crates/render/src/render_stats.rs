//! Per-frame renderer counters for manual UI performance traces.
//!
//! The renderer does not write trace files directly; `ui` snapshots
//! [`RenderStats`] around `Renderer::draw_buffer*` when
//! `CONTINUITY_UI_TRACE` is enabled and emits one aggregate line per
//! paint.
//!
//! [`RendererDrawStages`] (the chrome-overlay split that
//! `event:renderer_draw_stages` consumes) and
//! [`RendererChromeOverlayBreakdown`] (the renderer-side
//! per-sub-stage measurement struct) live in sibling modules so this
//! file stays under the 600-line conventions cap.

pub(crate) mod chrome_overlay_breakdown;
pub(crate) mod draw_stages;

use continuity_layout::LayoutCacheCounters;
use ropey::Rope;

use crate::params::DrawParams;
use crate::table_chrome_cache::TableChromePathStats;

use chrome_overlay_breakdown::RendererChromeOverlayBreakdown;
use draw_stages::RendererDrawStages;

/// Static chrome command-list path used by the most recent paint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChromePathMode {
    /// The command list was rebuilt before replay.
    Fresh,
    /// The cached command list was replayed without rebuilding.
    #[default]
    Replay,
}

impl ChromePathMode {
    /// Lowercase token emitted in `event:chrome_path`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Replay => "replay",
        }
    }
}

/// Static chrome record/replay timing from one paint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChromePathStats {
    /// Whether this paint rebuilt or reused the command list.
    pub mode: ChromePathMode,
    /// Microseconds spent in the command-list path for this paint.
    pub elapsed_us: u64,
}

impl ChromePathStats {
    /// Build a stats value for the supplied mode and elapsed time.
    #[must_use]
    pub const fn new(mode: ChromePathMode, elapsed_us: u64) -> Self {
        Self { mode, elapsed_us }
    }

    /// Format the TSV details column for `event:chrome_path`.
    #[must_use]
    pub fn trace_detail(self) -> String {
        format!("mode={} elapsed_us={}", self.mode.as_str(), self.elapsed_us)
    }
}

impl Default for ChromePathStats {
    fn default() -> Self {
        Self {
            mode: ChromePathMode::Replay,
            elapsed_us: 0,
        }
    }
}

/// Aggregate renderer work performed for one paint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderStats {
    /// `LayoutCache::get` calls that found an existing layout.
    pub layout_cache_hits: u64,
    /// `LayoutCache::get` calls that missed.
    pub layout_cache_misses: u64,
    /// Cached `IDWriteTextLayout`s inserted during the paint.
    pub layouts_created: u64,
    /// Cache misses where insertion did NOT trigger an eviction — i.e.
    /// the cache had spare capacity when the layout was built.
    pub layout_cache_miss_built: u64,
    /// Cache misses where insertion displaced a different LRU entry to
    /// make room. Non-zero means the cache is under pressure.
    pub layout_cache_miss_after_evict: u64,
    /// Microseconds spent in tree-sitter query walks during this
    /// paint's decoration resolve. `0` when not measured.
    pub tree_query_us: u64,
    /// Microseconds spent in decoration compute (span extraction,
    /// inline scan, table evaluate) during this paint. `0` when not
    /// measured.
    pub decoration_compute_us: u64,
    /// Focused-pane display rows with realized specs and glyph draw attempts.
    pub display_rows_drawn: u32,
    /// Focused-pane source lines visited by the body text pass.
    pub source_lines_visited: u32,
    /// Focused-pane soft-wrap continuation rows drawn.
    pub soft_wrap_continuation_rows_drawn: u32,
    /// Non-focused pane bodies painted after the focused pane.
    pub spectator_panes: u32,
    /// Source lines visited across non-focused pane body text passes.
    pub spectator_source_lines_visited: u32,
    /// Spell spans supplied to the renderer for this paint.
    pub spell_spans: u32,
    /// Focused-pane pipe-table visual layouts supplied to the renderer.
    pub table_layouts: u32,
    /// Focused-pane inline-image placements supplied to the renderer.
    pub image_placements: u32,
    /// `true` when the scaled-text minimap pass was enabled.
    pub minimap_enabled: bool,
    /// `true` when outline-sidebar paint data was supplied.
    pub outline_enabled: bool,
    /// `true` when status-bar paint data was supplied.
    pub status_bar_enabled: bool,
    /// Microseconds spent inside the body paint pass (wrap_paint or
    /// line_text_pass). Populated by [`Renderer::draw_buffer_no_present`]
    /// when the caller passes a `&mut RenderStats` and tracing is on.
    /// `0` when not measured.
    pub body_paint_us: u64,
    /// Microseconds spent inside the post-body paint pass (status bar,
    /// chrome, scrollbar, line numbers, minimap, search strip,
    /// inline-image overlays). Populated under the same conditions as
    /// `body_paint_us`.
    pub post_body_paint_us: u64,
    /// Post-body sub-stage timings from the most recent paint.
    pub post_body_stages: RendererPostBodyStages,
    /// Static chrome command-list path from the most recent paint.
    pub chrome_path: ChromePathStats,
    /// Per-table chrome command-list path from the most recent paint
    /// (P14.1).
    pub table_chrome_path: TableChromePathStats,
    /// Chrome-overlay sub-stage timings from the most recent paint.
    /// Populated by UI from `Renderer::last_chrome_overlay_breakdown`;
    /// feeds the chrome-overlay split fields on
    /// `event:renderer_draw_stages`.
    pub chrome_overlay_breakdown: RendererChromeOverlayBreakdown,
}

/// Soft-wrap overflow detector sample from the most recent focused-pane
/// wrap paint.
///
/// A display row "overflows" when its painted visible-glyph advance
/// (`DWRITE_TEXT_METRICS.width`, measured after bold / heading styling is
/// applied to the layout) extends past the text column's right edge even
/// though the soft-wrap pass decided the row fit. Diagnostic only:
/// populated every wrap paint, surfaced to the UI via
/// [`crate::Renderer::last_soft_wrap_overflow`], and emitted as
/// `event:soft_wrap_overflow` when tracing is enabled.
///
/// Not part of [`RenderStats`] because that struct derives `Eq`, which
/// `f32` fields would break; this rides the renderer's `last_*` accessor
/// pattern instead.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoftWrapOverflowSample {
    /// Visible display rows this paint whose advance exceeded the column.
    pub rows: u32,
    /// Source line of the worst (largest-overflow) row.
    pub worst_source_line: u32,
    /// Display-row index of the worst row.
    pub worst_display_row: u32,
    /// Painted visible advance of the worst row, in DIPs.
    pub worst_advance_dip: f32,
    /// Text-column width the worst row was wrapped against, in DIPs.
    pub worst_wrap_width_dip: f32,
    /// How far the worst row's advance ran past the column, in DIPs.
    pub worst_overflow_dip: f32,
    /// `true` when the worst row is a soft-wrap continuation (hang-
    /// indented). A continuation whose `worst_overflow_dip` ≈
    /// `worst_leading_dip` confirms the hang-indent-vs-full-width wrap
    /// mismatch (the break ignores the continuation's leading indent).
    pub worst_is_continuation: bool,
    /// Hang-indent (leading-whitespace advance) applied to the worst row
    /// when painting, in DIPs. Zero for non-continuation rows.
    pub worst_leading_dip: f32,
}

/// Post-body renderer sub-stage durations for one paint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererPostBodyStages {
    /// Brush construction for post-body chrome surfaces.
    pub brush_setup_us: u64,
    /// Non-focused pane body paint.
    pub spectator_bodies_us: u64,
    /// Jump glow and edit pulse overlays.
    pub motion_overlays_us: u64,
    /// Pane borders, tab strips, and pane labels.
    pub pane_chrome_us: u64,
    /// Status bar paint.
    pub status_bar_us: u64,
    /// Outline sidebar paint.
    pub outline_us: u64,
    /// Focused and spectator inline-image paint.
    pub inline_images_us: u64,
    /// Time-machine HUD paint.
    pub hud_us: u64,
    /// Focused-pane scrollbar paint.
    pub scrollbar_us: u64,
    /// Modal overlays and chord HUD paint.
    pub modal_overlays_us: u64,
    /// Enclosing post-body duration measured inside the renderer.
    pub total_us: u64,
}

impl RendererPostBodyStages {
    /// Sum every named sub-stage except the enclosing total.
    #[must_use]
    pub fn stage_sum_us(self) -> u64 {
        self.brush_setup_us
            .saturating_add(self.spectator_bodies_us)
            .saturating_add(self.motion_overlays_us)
            .saturating_add(self.pane_chrome_us)
            .saturating_add(self.status_bar_us)
            .saturating_add(self.outline_us)
            .saturating_add(self.inline_images_us)
            .saturating_add(self.hud_us)
            .saturating_add(self.scrollbar_us)
            .saturating_add(self.modal_overlays_us)
    }

    /// Format the TSV details column for `event:renderer_post_body_stages`.
    #[must_use]
    pub fn trace_detail(self) -> String {
        format!(
            concat!(
                "brush_setup_us={} spectator_bodies_us={} motion_overlays_us={} ",
                "pane_chrome_us={} status_bar_us={} outline_us={} inline_images_us={} ",
                "hud_us={} scrollbar_us={} modal_overlays_us={} stage_sum_us={} total_us={}"
            ),
            self.brush_setup_us,
            self.spectator_bodies_us,
            self.motion_overlays_us,
            self.pane_chrome_us,
            self.status_bar_us,
            self.outline_us,
            self.inline_images_us,
            self.hud_us,
            self.scrollbar_us,
            self.modal_overlays_us,
            self.stage_sum_us(),
            self.total_us,
        )
    }
}

impl RenderStats {
    /// Compute cheap row/feature counts from the immutable draw inputs.
    #[must_use]
    pub fn from_draw_params(rope: &Rope, params: &DrawParams<'_>) -> Self {
        let mut stats = Self {
            spell_spans: params.spell_spans.len() as u32,
            table_layouts: params.table_layouts.len() as u32,
            image_placements: params.images.map_or(0, |images| images.len() as u32),
            minimap_enabled: params.view_options.minimap,
            outline_enabled: params.outline.is_some(),
            status_bar_enabled: params.status_bar.is_some(),
            ..Self::default()
        };
        stats.count_focused_body_rows(rope, params);
        stats.count_spectator_body_rows(params);
        stats
    }

    /// Add layout-cache counter deltas captured around renderer submit.
    pub fn add_layout_cache_delta(&mut self, delta: LayoutCacheCounters) {
        self.layout_cache_hits = delta.hits;
        self.layout_cache_misses = delta.misses;
        self.layouts_created = delta.layouts_created;
        self.layout_cache_miss_after_evict = delta.layouts_created_after_evict;
        self.layout_cache_miss_built = delta
            .layouts_created
            .saturating_sub(delta.layouts_created_after_evict);
    }

    /// Format the TSV details column for `paint:render_stats`.
    ///
    /// `layout_hits` and `layouts_created` are kept under their original
    /// names for backward-compatible parsing (the xtask analyze-trace
    /// surface and external trace tooling key on them); the new
    /// `layout_cache_hits` / `layout_cache_miss_built` /
    /// `layout_cache_miss_after_evict` aliases land alongside so future
    /// consumers can switch over.
    #[must_use]
    pub fn trace_detail(&self) -> String {
        format!(
            concat!(
                "layout_hits={} layout_misses={} layouts_created={} ",
                "layout_cache_hits={} layout_cache_miss_built={} ",
                "layout_cache_miss_after_evict={} ",
                "display_rows_drawn={} source_lines_visited={} ",
                "soft_wrap_continuations={} spectator_panes={} spectator_source_lines={} ",
                "spell_spans={} table_layouts={} image_placements={} ",
                "minimap={} outline={} status_bar={} ",
                "body_paint_us={} post_body_paint_us={} ",
                "tree_query_us={} decoration_compute_us={}"
            ),
            self.layout_cache_hits,
            self.layout_cache_misses,
            self.layouts_created,
            self.layout_cache_hits,
            self.layout_cache_miss_built,
            self.layout_cache_miss_after_evict,
            self.display_rows_drawn,
            self.source_lines_visited,
            self.soft_wrap_continuation_rows_drawn,
            self.spectator_panes,
            self.spectator_source_lines_visited,
            self.spell_spans,
            self.table_layouts,
            self.image_placements,
            self.minimap_enabled,
            self.outline_enabled,
            self.status_bar_enabled,
            self.body_paint_us,
            self.post_body_paint_us,
            self.tree_query_us,
            self.decoration_compute_us,
        )
    }

    /// Draw-stage split derived from the renderer's per-pass timers.
    /// `chrome_overlay_us` is derived from the renderer-side breakdown
    /// sum rather than `post_body_paint_us` so the chrome-overlay
    /// accounting contract (`chrome_overlay_sum_us ≈
    /// chrome_overlay_us`) holds by construction. The fold in
    /// [`Self::draw_stages_for_enclosing`] applies any remaining gap
    /// to both fields equally, preserving the parity.
    #[must_use]
    pub fn draw_stages(&self) -> RendererDrawStages {
        let breakdown = self.chrome_overlay_breakdown;
        let chrome_overlay_us = breakdown.sum_us();
        RendererDrawStages {
            body_glyphs_us: self.body_paint_us,
            chrome_overlay_us,
            chrome_overlay_table_us: self.table_chrome_path.elapsed_us(),
            chrome_overlay_line_numbers_us: breakdown.line_numbers_us,
            chrome_overlay_indent_guides_us: breakdown.indent_guides_us,
            chrome_overlay_selection_bars_us: breakdown.selection_bars_us,
            chrome_overlay_search_ticks_us: breakdown.search_ticks_us,
            chrome_overlay_block_backgrounds_us: breakdown.block_backgrounds_us,
            chrome_overlay_horizontal_rules_us: breakdown.horizontal_rules_us,
            chrome_overlay_code_copy_button_us: breakdown.code_copy_button_us,
            chrome_overlay_minimap_us: breakdown.minimap_us,
            chrome_overlay_outline_sidebar_us: breakdown.outline_sidebar_us,
            chrome_overlay_scrollbar_us: breakdown.scrollbar_us,
            chrome_overlay_decoration_us: breakdown.decoration_us,
            ..RendererDrawStages::default()
        }
    }

    /// Draw-stage split adjusted to the enclosing renderer duration.
    /// Any overhead between the enclosing call and the sum of named
    /// top-level stages (body / chrome / table chrome) is folded into
    /// both `chrome_overlay_us` and the catch-all
    /// `chrome_overlay_decoration_us` sub-stage so the breakdown
    /// accounting contract still holds.
    #[must_use]
    pub fn draw_stages_for_enclosing(&self, enclosing_us: Option<u64>) -> RendererDrawStages {
        let mut stages = self.draw_stages();
        if let Some(enclosing_us) = enclosing_us {
            let stage_sum = stages.total_us();
            if enclosing_us > stage_sum {
                let gap = enclosing_us.saturating_sub(stage_sum);
                stages.chrome_overlay_us = stages.chrome_overlay_us.saturating_add(gap);
                stages.chrome_overlay_decoration_us =
                    stages.chrome_overlay_decoration_us.saturating_add(gap);
            }
        }
        stages
    }

    fn count_focused_body_rows(&mut self, rope: &Rope, params: &DrawParams<'_>) {
        let line_height = params.line_height.max(1.0);
        let viewport_h = params.view.viewport_height_dip.max(1.0);
        let scroll_y = params.view.scroll_y_dip;
        if params.view.soft_wrap {
            let total_display = params.frame_display.display_line_count() as i64;
            let first = ((scroll_y / line_height).floor() as i64).max(0) as u32;
            let last = ((((scroll_y + viewport_h) / line_height).ceil() as i64) + 1)
                .clamp(0, total_display) as u32;
            let mut last_source_line: Option<u32> = None;
            for display_row in first..last {
                let Some(spec) = params.frame_display.display_line_by_index(display_row) else {
                    continue;
                };
                self.display_rows_drawn = self.display_rows_drawn.saturating_add(1);
                let source_line = spec.source_line.raw();
                if last_source_line != Some(source_line) {
                    self.source_lines_visited = self.source_lines_visited.saturating_add(1);
                    last_source_line = Some(source_line);
                }
                if spec.is_wrap_continuation {
                    self.soft_wrap_continuation_rows_drawn =
                        self.soft_wrap_continuation_rows_drawn.saturating_add(1);
                }
            }
        } else {
            let total_lines = rope.len_lines().max(1);
            let first = ((scroll_y / line_height).floor() as isize).max(0) as usize;
            let last =
                (((scroll_y + viewport_h) / line_height).ceil() as usize + 1).min(total_lines);
            for line_idx in first..last {
                if params.frame_display.line(line_idx).is_some() {
                    self.display_rows_drawn = self.display_rows_drawn.saturating_add(1);
                    self.source_lines_visited = self.source_lines_visited.saturating_add(1);
                }
            }
        }
    }

    fn count_spectator_body_rows(&mut self, params: &DrawParams<'_>) {
        self.spectator_panes = params.pane_bodies.len() as u32;
        for body in params.pane_bodies {
            let (_, _, _, height) = body.rect;
            if height <= 0.0 {
                continue;
            }
            let line_height = params.line_height.max(1.0);
            let viewport_h = body.view.viewport_height_dip.max(height);
            let scroll_y = body.view.scroll_y_dip;
            let total_lines = body.rope.len_lines().max(1);
            let first = ((scroll_y / line_height).floor() as isize).max(0) as usize;
            let last =
                (((scroll_y + viewport_h) / line_height).ceil() as usize + 1).min(total_lines);
            let Some(frame_display) = body.frame_display else {
                self.spectator_source_lines_visited = self
                    .spectator_source_lines_visited
                    .saturating_add(last.saturating_sub(first) as u32);
                continue;
            };
            for line_idx in first..last {
                if frame_display.line(line_idx).is_some() {
                    self.spectator_source_lines_visited =
                        self.spectator_source_lines_visited.saturating_add(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_detail_names_manual_perf_fields() {
        let mut stats = RenderStats {
            display_rows_drawn: 12,
            source_lines_visited: 9,
            soft_wrap_continuation_rows_drawn: 3,
            spectator_panes: 2,
            spectator_source_lines_visited: 18,
            spell_spans: 4,
            table_layouts: 1,
            image_placements: 5,
            minimap_enabled: true,
            outline_enabled: false,
            status_bar_enabled: true,
            ..RenderStats::default()
        };
        stats.add_layout_cache_delta(LayoutCacheCounters {
            hits: 7,
            misses: 2,
            layouts_created: 2,
            layouts_created_after_evict: 0,
        });

        assert_eq!(
            stats.trace_detail(),
            "layout_hits=7 layout_misses=2 layouts_created=2 \
layout_cache_hits=7 layout_cache_miss_built=2 layout_cache_miss_after_evict=0 \
display_rows_drawn=12 source_lines_visited=9 soft_wrap_continuations=3 \
spectator_panes=2 spectator_source_lines=18 spell_spans=4 table_layouts=1 \
image_placements=5 minimap=true outline=false status_bar=true \
body_paint_us=0 post_body_paint_us=0 \
tree_query_us=0 decoration_compute_us=0"
        );
    }

    #[test]
    fn chrome_path_trace_detail_names_mode_and_elapsed_time() {
        let stats = ChromePathStats::new(ChromePathMode::Fresh, 42);
        assert_eq!(stats.trace_detail(), "mode=fresh elapsed_us=42");
    }
}
