//! Per-frame [`ViewOptionsDraw`] projection — converts the UI thread's
//! `view_options` + active theme + per-frame heading list into the
//! renderer's view-options payload, including the focus-mode dim
//! parameters resolved from `[focus].dim_alpha` (when non-zero) with
//! fallback to the theme's `editor.focus_dim_alpha` key.

use continuity_render::{Rgba, ViewOptionsDraw};

use super::caret_shape::caret_shape_for;
use crate::window::Window;

impl Window {
    /// Build the renderer payload. The three borrowed slots
    /// (`ruler_columns`, `focus_mode`, `folded_lines`) take *narrow*
    /// sub-field references so the returned struct's lifetime is
    /// pinned only to those three fields — leaving the rest of
    /// `self.view_options` free to be mutated by downstream
    /// `&mut self` calls (notably the status-bar-layout cache
    /// assignment that runs later in `on_paint`).
    pub(crate) fn build_view_options_draw<'a>(
        &self,
        ruler_columns: &'a [u32],
        focus_mode: &'static str,
        folded_lines: &'a [u32],
        scaled_font_size: f32,
        search_minimap_active: bool,
        heading_lines_for_folds: &'a [(u32, u8)],
    ) -> ViewOptionsDraw<'a> {
        // §H1 — resolve focus-mode dim alpha + color. `[focus].dim_alpha`
        // (already loaded into `pane_modes.focus_dim_alpha`) wins when
        // non-zero; otherwise fall through to the theme key's alpha
        // channel. RGB always comes from `editor.foreground_dim`.
        let focus_dim_color = {
            let c = self.active_theme.editor_foreground_dim();
            Rgba {
                r: (c.r as f32) / 255.0,
                g: (c.g as f32) / 255.0,
                b: (c.b as f32) / 255.0,
                a: 1.0,
            }
        };
        let focus_dim_alpha = if self.view_options.pane_modes.focus_dim_alpha > f32::EPSILON {
            self.view_options.pane_modes.focus_dim_alpha
        } else {
            (self.active_theme.editor_focus_dim_alpha().a as f32) / 255.0
        };
        ViewOptionsDraw {
            line_numbers: self.view_options.line_numbers,
            // Gutter expands from caret-line-only to full line numbers
            // while the cursor hovers over the gutter strip.
            gutter_caret_line_only: self.view_options.gutter_caret_line_only
                && !self.mouse_state.gutter_hovered,
            relative_line_numbers: self.view_options.relative_line_numbers,
            current_line_highlight: self.view_options.current_line_highlight,
            indent_guides: self.view_options.indent_guides,
            whitespace_markers: self.view_options.whitespace_markers,
            trailing_whitespace: self.view_options.trailing_whitespace,
            minimap: self.view_options.minimap,
            indent_size: self.view_options.indent_size,
            tab_width: self.view_options.tab_width,
            ruler_columns,
            caret_shape: caret_shape_for(self.view_options.caret_style),
            caret_visible: self.caret_blink_visible,
            caret_bar_width_px: self.view_options.caret_width_px,
            show_status_bar: self.view_options.show_status_bar,
            show_sticky_breadcrumb: self.view_options.show_sticky_breadcrumb,
            show_outline_sidebar: self.view_options.show_outline_sidebar,
            outline_sidebar_width_dip: self.view_options.outline_sidebar_width_dip,
            search_minimap_active,
            show_tab_strip: self.view_options.show_tab_strip,
            show_pane_borders: self.view_options.show_pane_borders,
            distraction_free: self.view_options.pane_modes.distraction_free,
            distraction_free_max_width_dip: self.view_options.pane_modes.distraction_free_max_width
                as f32
                * scaled_font_size
                * 0.55,
            // §H1 — focus-mode dim pass.
            focus_mode,
            focus_dim_alpha,
            focus_dim_color,
            // §H3 — gutter triangle painter consumes the toggled set.
            folded_lines,
            // §H3 heading folds — the painter recognizes heading lines
            // as foldable and computes the heading-fold extent for the
            // gutter "▸ N" indicator.
            markdown_headings: heading_lines_for_folds,
            // Markdown render-paint gates mirrored from
            // `[markdown].render_highlight` / `render_divider`. The
            // display map handles the marker-hide side; these flags gate
            // the highlight background fill and the horizontal-rule line.
            render_highlight_bg: self.markdown_render_toggles().highlight,
            render_divider: self.markdown_render_toggles().divider,
        }
    }
}
