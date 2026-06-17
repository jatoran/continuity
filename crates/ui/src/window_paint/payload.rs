//! Per-frame chrome payload builders: scaled/search minimaps, outline
//! sidebar, and pipe-table visual layouts. Each helper returns owned data that
//! the renderer borrows from `DrawParams` later in the paint pipeline.
//! Layout caches (`view_options.minimap_layout`,
//! `view_options.search_minimap_layout`, `view_options.outline_layout`)
//! are written here so the next mouse hit-test runs against the same
//! geometry the painter consumed.

use continuity_decorate::{DecorationCache, Decorations};
use continuity_render::{
    compute_table_layouts_with_overrides, FrameDisplay, InlineImagePlacement, OutlineColors,
    OutlineData, OutlineEntry, PaneBodyDraw, SearchMinimapDraw, TableColWidthOverride, TableLayout,
};
use continuity_text::Selection;
use ropey::Rope;

use crate::pane_layout::Rect;
use crate::window::Window;
use crate::window_outline_entries_cache::{
    build_outline_entries_snapshot, OutlineEntriesCacheKey, OutlineEntriesSnapshot,
};
use crate::window_paint_builders::{compute_outline_current_index, NonFocusedPaneRender};

impl Window {
    /// Phase G4: build + cache the search minimap strip layout. The
    /// strip insets by the outline-sidebar width when both are active
    /// so the two right-edge columns don't share x-pixels.
    pub(crate) fn build_search_minimap_payload(
        &mut self,
        body_rect: &Rect,
        snap_rope: &Rope,
        search_minimap_active: bool,
        editor_colors: continuity_render::EditorColors,
    ) -> Option<SearchMinimapDraw> {
        if !search_minimap_active {
            self.view_options.search_minimap_layout = None;
            return None;
        }
        let fb = self.overlays.find_bar().expect("active iff some");
        let total_lines = snap_rope.len_lines().max(1) as u64;
        let strip_right_inset = if self.view_options.show_outline_sidebar {
            self.view_options.outline_sidebar_width_dip.max(0.0)
        } else {
            0.0
        };
        let layout = crate::search_minimap::build_layout(
            body_rect.w,
            body_rect.h,
            total_lines,
            &fb.matches,
            fb.current,
            strip_right_inset,
        );
        let payload = crate::search_minimap::project_to_draw(
            &layout,
            &editor_colors,
            &fb.matches,
            fb.current,
        );
        self.view_options.search_minimap_layout = Some(layout);
        Some(payload)
    }

    /// Phase F2 outline-sidebar entries + colors + current row index.
    /// Returns empty `entries` when the sidebar toggle is off so the
    /// caller can skip the renderer's outline paint pass cleanly.
    pub(crate) fn build_outline_payload_pieces(
        &self,
        rope_revision: u64,
        snap_rope: &Rope,
        snap_selections: &[Selection],
        decorations: Option<&Decorations>,
    ) -> (Vec<OutlineEntry>, OutlineColors, Option<u32>) {
        let snapshot: OutlineEntriesSnapshot = if self.view_options.show_outline_sidebar {
            let key = OutlineEntriesCacheKey {
                buffer_id: self.buffer_id,
                rope_revision,
                decoration_revision: decorations.map(|decorations| decorations.revision),
            };
            let started = crate::paint_trace::is_trace_enabled().then(std::time::Instant::now);
            let (snapshot, status) = self
                .outline_entries_cache
                .borrow_mut()
                .get_or_build(key, || {
                    build_outline_entries_snapshot(snap_rope, decorations)
                });
            if let Some(started) = started {
                crate::paint_trace::log_event(
                    "event:outline_entries",
                    &format!(
                        "cache={} rope_rev={} decoration_rev={} entries={} elapsed_us={}",
                        status.as_str(),
                        rope_revision,
                        decorations
                            .map(|decorations| decorations.revision.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        snapshot.entries.len(),
                        started.elapsed().as_micros(),
                    ),
                );
            }
            snapshot
        } else {
            OutlineEntriesSnapshot::default()
        };
        let colors = OutlineColors {
            bg: crate::window_theme::rgba_from_color(
                self.active_theme.current.editor_outline_background(),
            ),
            fg: crate::window_theme::rgba_from_color(
                self.active_theme.current.editor_outline_foreground(),
            ),
            fg_active: crate::window_theme::rgba_from_color(
                self.active_theme.current.editor_outline_foreground_active(),
            ),
            separator: crate::window_theme::rgba_from_color(
                self.active_theme.current.editor_outline_separator(),
            ),
        };
        let current_index =
            compute_outline_current_index(&snapshot.headings, snap_rope, snap_selections);
        (snapshot.entries, colors, current_index)
    }

    /// Cache the outline-sidebar row-rect layout so the next click
    /// hit-tests against the same geometry the painter consumed.
    /// Pane rect matches the one the renderer derives from
    /// `body_origin` + viewport dims.
    pub(crate) fn cache_outline_layout(
        &mut self,
        outline_data: Option<&OutlineData<'_>>,
        body_origin: (f32, f32),
    ) {
        let layout = outline_data.map(|d| {
            let pane_rect = (
                body_origin.0,
                body_origin.1,
                self.view.viewport_width_dip.max(1.0),
                self.view.viewport_height_dip.max(1.0),
            );
            let layout = continuity_render::compute_outline_layout(
                d,
                pane_rect,
                self.view_options.outline_scroll_offset_dip,
            );
            self.view_options.outline_scroll_offset_dip = layout.scroll_offset_dip;
            layout
        });
        self.view_options.outline_layout = layout;
    }

    /// Cache the scaled-text minimap layout so click and drag hit-tests
    /// use the same geometry as the paint pass.
    pub(crate) fn cache_scaled_minimap_layout(&mut self, snap_rope: &Rope) {
        if !self.view_options.minimap {
            self.view_options.minimap_layout = None;
            return;
        }
        let outline_inset = if self.view_options.show_outline_sidebar {
            self.view_options.outline_sidebar_width_dip.max(0.0)
        } else {
            0.0
        };
        let pane_rect = (
            0.0,
            0.0,
            self.view.viewport_width_dip.max(1.0),
            self.view.viewport_height_dip.max(1.0),
        );
        self.view_options.minimap_layout = Some(continuity_render::compute_minimap_layout(
            pane_rect,
            self.view.scroll_y_dip,
            self.effective_line_height(),
            snap_rope.len_lines().max(1) as u64,
            self.estimated_content_height(),
            outline_inset,
        ));
    }

    /// Document-absolute `EvaluatedTable.block_range`s of every table
    /// the active selection has reached past a single cell of —
    /// passed to both the display-map builder (skips hide emission)
    /// and the chrome painter (skips visual layout). Empty when:
    /// - there's no decoration snapshot for the active buffer, or
    /// - every selection is a caret (collapsed), or
    /// - every selection fits inside a single cell's content range
    ///   (no pipe bytes covered).
    ///
    /// Used by `Window::on_paint` and `window_mouse_segment_hit`'s
    /// per-line projection stamp so the segment cache busts on
    /// selection-vs-table overlap flips.
    pub(crate) fn compute_suppressed_table_blocks(&self) -> Vec<std::ops::Range<usize>> {
        let Some(snap) = self.editor.snapshot(self.buffer_id) else {
            return Vec::new();
        };
        let id = self.buffer_id.as_uuid().as_u128();
        let Some(dec) = self.decoration_cache.get(id) else {
            return Vec::new();
        };
        continuity_render::compute_suppressed_table_blocks(
            snap.rope_snapshot().rope(),
            snap.selections(),
            &dec.evaluated_tables,
        )
    }

    /// Pipe-table visual layouts — one [`TableLayout`] per pipe-table
    /// block. Empty when no decoration snapshot is available. Column
    /// widths use a monospace approximation
    /// (`chars * projection_char_width`) so widths align with the body
    /// glyphs the display map projects after pipe hiding. Tables are
    /// laid out unconditionally; caret position no longer gates
    /// rendering.
    ///
    /// Caches the most recent non-empty result in
    /// `Window::last_focused_table_layouts` and falls back to it when
    /// the current build comes back empty. The empty-result case
    /// typically happens on the one-frame gap between a keystroke and
    /// the decorate worker delivering an updated parse:
    /// `build_one_table_layout`'s byte-to-char guard rejects the stale
    /// `block_range` against the fresh rope and the table drops out
    /// of `evaluated_tables`. Without the fallback the user sees the
    /// chrome flash off and on every character they type.
    pub(crate) fn build_focused_pane_table_layouts(
        &self,
        decorations: Option<&Decorations>,
        rope_for_projection: &Rope,
        caret_bytes: &[usize],
        suppressed_table_blocks: &[std::ops::Range<usize>],
        projection_char_width: f32,
    ) -> std::sync::Arc<Vec<TableLayout>> {
        // Bounds-validate every decorated table against the current
        // rope length. The two interesting empty-result cases differ
        // by which direction the rope moved relative to the
        // decoration snapshot:
        //
        // - Typing-lag (rope GREW past `block_range.end`): every
        //   table's range is still entirely inside the current rope.
        //   `compute_table_layouts` can transiently return `None`
        //   for a multi-byte-char misalignment; the cached prior
        //   layout is the right fallback so chrome paints
        //   continuously instead of flashing on/off per keystroke.
        //
        // - Delete-table-lag (rope SHRUNK below `block_range.end`):
        //   at least one table's range no longer fits inside the
        //   rope. The cached prior layout describes content that
        //   was deleted; trusting it paints ghost chrome over now-
        //   blank source bytes. Drop the cache and return empty.
        //
        // `any_table_in_bounds` distinguishes the two: true iff at
        // least one table's `block_range` fits entirely inside the
        // current rope, which only the typing-lag case satisfies.
        let rope_len = rope_for_projection.len_bytes();
        let any_table_in_bounds = decorations.is_some_and(|d| {
            d.evaluated_tables
                .iter()
                .any(|t| t.block_range.end <= rope_len && t.block_range.start < rope_len)
        });
        let fresh = if let Some(d) = decorations {
            // Phase F — a live column-resize drag injects a transient
            // width override so the dragged column previews at the new
            // width (and wrapping / row reservations reflow) before the
            // width is committed to the directive on release.
            let overrides: Vec<TableColWidthOverride> =
                self.active_table_col_override().into_iter().collect();
            let mut measure = |text: &str| text.chars().count() as f32 * projection_char_width;
            compute_table_layouts_with_overrides(
                &d.evaluated_tables,
                rope_for_projection,
                caret_bytes,
                suppressed_table_blocks,
                &overrides,
                &mut measure,
            )
        } else {
            Vec::new()
        };
        if !fresh.is_empty() {
            // Wrap in Arc before caching: both the cache insert AND
            // the return become refcount bumps rather than deep
            // clones of the cell vector. TableLayout owns `String`
            // cell text whose deep-clone otherwise dominates the
            // per-keystroke paint cost on large tables.
            let arc = std::sync::Arc::new(fresh);
            self.last_focused_table_layouts
                .borrow_mut()
                .insert(self.buffer_id, std::sync::Arc::clone(&arc));
            return arc;
        }
        if !any_table_in_bounds {
            // Delete-table-lag path. Either there are no decorated
            // tables at all (genuine empty) or every decorated
            // table's range extends past the current rope (the
            // worker hasn't re-parsed yet after a delete that
            // shrunk the rope). Drop the cache so the next paint
            // shows the post-delete state and not the deleted
            // table's chrome.
            self.last_focused_table_layouts
                .borrow_mut()
                .remove(&self.buffer_id);
            return std::sync::Arc::new(Vec::new());
        }
        // Typing-lag path. Cached prior layout describes content
        // that's still in the rope (just one revision behind);
        // reuse it so chrome paints continuously instead of
        // flashing on/off per keystroke.
        if let Some(cached) = self
            .last_focused_table_layouts
            .borrow()
            .get(&self.buffer_id)
        {
            return std::sync::Arc::clone(cached);
        }
        std::sync::Arc::new(Vec::new())
    }
}

/// Build the per-frame `PaneBodyDraw` list for all non-focused
/// (spectator) panes. The function takes only the narrow sub-field
/// borrows the closure actually needs — `&self.decoration_cache` plus
/// the three per-pane spectator vectors — so the result's lifetime
/// pins to those fields and not the whole `Window`, leaving
/// `&mut self.cache` free for the renderer dispatch below.
pub(crate) fn build_pane_bodies<'a>(
    other_panes: &'a [NonFocusedPaneRender],
    decoration_cache: &'a DecorationCache,
    pane_table_layouts: &'a [Vec<TableLayout>],
    pane_frame_displays: &'a [FrameDisplay],
    pane_image_placements: &'a [Vec<InlineImagePlacement>],
) -> Vec<PaneBodyDraw<'a>> {
    other_panes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let dec = decoration_cache.get(p.document);
            PaneBodyDraw {
                document: p.document,
                rect: (p.rect.0, p.rect.1, p.rect.2, p.rect.3),
                rope: p.snapshot.rope_snapshot().rope(),
                selections: p.snapshot.selections(),
                view: &p.view,
                decorations: dec,
                inline_color_spans: dec.map(|d| d.inline_color_spans.as_slice()).unwrap_or(&[]),
                table_overrides: dec.map(|d| d.evaluated_tables.as_slice()).unwrap_or(&[]),
                table_layouts: &pane_table_layouts[i],
                frame_display: Some(&pane_frame_displays[i]),
                images: &pane_image_placements[i],
                minimap: p.minimap,
                show_outline_sidebar: p.show_outline_sidebar,
                is_focused: false,
            }
        })
        .collect()
}
