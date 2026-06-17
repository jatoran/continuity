//! `Window::on_paint` orchestrator and per-frame `DrawParams` wiring.
//! Thread ownership: UI thread of one window.

use continuity_render::{
    DrawParams, LineHoverDraw, OutlineData, StatusBarData, DEFAULT_HEADING_SCALE,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::display_prewarm_cache::PrewarmQuery;
use crate::overlay_render::build_overlay_draw;
use crate::paint_trace::PaintTrace;
use crate::window::Window;
use crate::window_code_copy_hover::build_code_copy_button_draw;
use crate::Error;

mod cache_seed;
mod caret_shape;
mod cold_deferred;
mod decorations;
mod dispatch;
mod doc_end_scroll;
mod epilogue;
mod frame_resolution;
mod mouse_candidate;
mod offthread_jump;
pub(crate) mod payload;
mod projection_stale_trace;
mod snapshot;
mod view_options;

// Keep the long-standing `crate::window_paint::...` viewport-row import path stable.
pub(crate) use crate::window_paint_viewport_rows::{
    visible_display_row_range, VIEWPORT_OVERSCAN_ROWS,
};

impl Window {
    pub(crate) fn on_paint(&mut self, hwnd: HWND) -> Result<(), Error> {
        if !self.begin_paint_frame(hwnd)? {
            return Ok(());
        }
        let Some(snap) = self.snapshot_for_paint()? else {
            return Ok(());
        };
        // Phase G4: strip active only while find bar is open + non-empty.
        let search_minimap_active = self
            .overlays
            .find_bar()
            .is_some_and(|fb| !fb.matches.is_empty());
        let rope_for_projection = snap.rope_snapshot().rope();
        let caret_bytes_for_projection =
            Self::caret_bytes_for_projection(rope_for_projection, snap.selections());
        let revision_for_projection = snap.rope_snapshot().revision().0;
        crate::paint_trace::log_paint_prologue();
        self.trace_paint_window_state(&snap);
        let trace = PaintTrace::new(
            rope_for_projection.len_lines() as u32,
            revision_for_projection,
        );
        trace.mark("snapshot+caret_bytes");
        let projection_metrics =
            self.display_projection_metrics(search_minimap_active, rope_for_projection.len_lines());
        trace.mark("projection_metrics");
        let undecorated_folds =
            self.display_projection_folds(rope_for_projection, &[], &caret_bytes_for_projection);
        trace.mark("undecorated_folds");
        let decoration_id = self.buffer_id.as_uuid().as_u128();
        let has_any_decorations = self.decoration_cache.get(decoration_id).is_some();
        let undecorated_query = PrewarmQuery::new(
            self.buffer_id,
            revision_for_projection,
            None,
            &caret_bytes_for_projection,
            &undecorated_folds,
            projection_metrics.wrap_width_dip,
            self.font_state,
        );
        // Only fall back to the undecorated prewarm path when the
        // cache has *no* decorations at all. Stale decorations are
        // transformed forward in `resolve_decorations_for_paint`.
        let mut prewarmed_frame_display = if has_any_decorations {
            None
        } else {
            self.prewarmed_frame_for_query(&undecorated_query, true)
        };
        // Close the typing-flicker gap: if the worker pool hasn't
        // produced decorations for the current rope revision yet,
        // recompute inline so the marker-hiding / bullet-glyph paint
        // doesn't fall back to stale byte ranges this frame.
        if prewarmed_frame_display.is_none() {
            self.ensure_fresh_decorations(&snap);
        }
        trace.mark("decorations_freshness");
        let document = self.buffer_id.as_uuid().as_u128();
        let overlay_draw = if self.overlays.is_active() {
            build_overlay_draw(
                &self.overlays,
                &self.keymap,
                &self.registry,
                self.client_width_dip(),
                self.client_height_dip(),
                self.overlay_input_focused,
            )
        } else if let Some(hover) =
            self.footnote_hover_overlay(self.client_width_dip(), self.client_height_dip())
        {
            Some(hover)
        } else {
            self.file_banner_overlay(self.client_width_dip())
        };
        let motion_now_ms = unsafe { GetTickCount64() };
        let mut should_start_motion_timer = false;
        let overlay_layer = self.project_overlay_layer(overlay_draw, motion_now_ms);
        let chord_hud_layer = self.project_chord_hud_layer(motion_now_ms);
        let jump_glow = self.jump_glow_draw(motion_now_ms);
        let edit_pulse = self.edit_pulse_draw(motion_now_ms);
        let format = self
            .text_format
            .as_ref()
            .expect("text format ready")
            .clone();
        let font_state = self.font_state;
        // Reuse stale cached decorations by transforming their byte ranges forward.
        let resolved_decorations =
            self.resolve_decorations_for_paint(decoration_id, revision_for_projection);
        let decorations_owned = resolved_decorations.owned;
        let current_decoration_parse_revision = resolved_decorations.current_parse_revision;
        let decoration_parse_advanced = resolved_decorations.parse_advanced;
        let decorations = decorations_owned.as_deref();
        let renderer = self.renderer.as_ref().expect("renderer ready");
        let scaled_font_size = self.scaled_font_size();
        let line_height = self.effective_line_height();
        let editor_colors = self.active_theme.editor_colors();
        let markdown_colors = self.active_theme.markdown_colors();
        // §H3 — build the per-frame markdown heading list `(line, level)`
        // up-front so both the fold provider call and the renderer's
        // `ViewOptionsDraw` can borrow it. Empty when no decorations
        // are available or the buffer has no headings.
        let heading_lines_for_folds = self.cached_heading_lines_for_projection(
            self.buffer_id,
            snap.rope_snapshot().rope(),
            snap.rope_snapshot().revision().get(),
            decorations,
        );
        trace.mark("heading_lines");
        // Phase 13: focused pane's body rect drives body_origin.
        let body_rect = self.focused_body_rect();
        let body_origin = (body_rect.x, body_rect.y);
        let mut chrome = build_pane_chrome(self);
        if let Some(chrome_draw) = chrome.as_mut() {
            self.chrome_motion.update(
                chrome_draw,
                self.motion_policy,
                &mut self.stagger_scheduler,
                motion_now_ms,
            );
            if self.chrome_motion.is_active(motion_now_ms) {
                should_start_motion_timer = true;
            }
        }
        // Phase 16.5: convert absolute spell-error byte ranges into
        // (line, byte_in_line) pairs the renderer can draw against the
        // active document's cached layouts. Spell-check off ⇒ empty
        // slice ⇒ zero squiggle paint.
        let spell_spans = build_spell_squiggle_spans(self, snap.rope_snapshot().rope());
        trace.mark("spell_spans");
        // Phase 16.5: snapshots + view state for every *non-focused*
        // pane leaf, so the renderer can paint each one's text in its
        // own clip rect. The Vec must outlive `pane_bodies` because
        // each `PaneBodyDraw` borrows into it.
        let mut other_panes: Vec<NonFocusedPaneRender> = collect_non_focused_panes(self);
        // Phase 17.6: build the per-frame display projection. The renderer
        // uses this to build each `IDWriteTextLayout` from the *visible*
        // display string — markers, fence ticks, and bullet-marker source
        // bytes never enter the layout. Without this, `**hi**` lays out as
        // 8 character widths even with markers hidden by paint-over.
        let projection_char_width = projection_metrics.char_width_dip;
        // Spectator-pane prep. Each spectator decides caret-in vs
        // caret-out using its OWN selection set, builds its OWN display
        // projection, and surfaces its OWN inline-image placements. The
        // four parallel `Vec`s share the spectator index and must
        // outlive `pane_bodies` below.
        let spectators = crate::window_paint_spectators::build_spectator_pane_data(
            self,
            &mut other_panes,
            projection_char_width,
            &mut |path| renderer.cached_image_dimensions(path),
        );
        trace.mark("spectators");
        let pane_table_layouts = &spectators.table_layouts;
        let pane_frame_displays = &spectators.frame_displays;
        let pane_image_placements = &spectators.image_placements;
        // `view_options` (the renderer payload) and `pane_bodies` are
        // built *just before* `DrawParams` so their immutable sub-field
        // borrows of self do not straddle the `&mut self` calls that
        // follow (resolve_paint_frame_display, build_search_minimap_payload,
        // cache_outline_layout, plus the status-bar-layout assignment).
        // §H3 / β — derive the exact fold set once, then either reuse
        // an idle-prewarmed frame for this key or build the projection
        // cold as before.
        let folds_for_projection = self.display_projection_folds(
            rope_for_projection,
            &heading_lines_for_folds,
            &caret_bytes_for_projection,
        );
        trace.mark("folds");
        // Pipe-table visual layouts — built HERE (before the frame
        // display) so their Phase F per-row heights can feed the
        // reservation set. Selection-suppressed tables (Ctrl+A,
        // Shift+arrow across pipes, drag-select across cells) unrender
        // so raw markdown shows; the set is shared with the display-map
        // hide pass. The resulting `Arc` is reused for the DrawParams
        // payload below, so the layout is computed once per paint.
        let suppressed_table_blocks = self.compute_suppressed_table_blocks();
        let table_layouts = self.build_focused_pane_table_layouts(
            decorations,
            rope_for_projection,
            &caret_bytes_for_projection,
            &suppressed_table_blocks,
            projection_char_width,
        );
        // γ / Phase F — image-row reservations for expanded inline
        // images, merged with the table-row reservations so a `<br>` /
        // wrapped table row reserves its extra display rows too. A
        // cold image cache emits no reservation (first paint after
        // expand falls back to one display row); the table rows are
        // always known from the layout. The merged set keys the frame
        // cache (`with_image_reservations` below) so a stable set hits.
        let image_reservations = self.compute_focused_pane_image_reservations(
            decorations,
            rope_for_projection,
            line_height,
            body_rect.w.max(1.0),
        );
        let image_reservations = crate::window_image_placements::merge_table_row_reservations(
            image_reservations,
            &table_layouts,
        );
        trace.mark("image_reservations");
        let display_query = PrewarmQuery::new(
            self.buffer_id,
            revision_for_projection,
            decorations.map(|decorations| decorations.revision),
            &caret_bytes_for_projection,
            &folds_for_projection,
            projection_metrics.wrap_width_dip,
            self.font_state,
        )
        .with_image_reservations(&image_reservations);
        // ε.2 — compute the absolute display-row range the painter will
        // iterate, so the cold-build path can materialize only those
        // source lines. Cache hits whose realized window does not cover
        // the viewport (e.g. after a scroll) fall through to a fresh
        // cold build.
        let viewport_rows = visible_display_row_range(
            self.view.scroll_y_dip,
            self.view.viewport_height_dip,
            line_height,
        );
        // Resolve which `FrameDisplay` to paint with: try motion-reuse
        // of the previous frame, then the prewarm cache, then the
        // projection worker, then an inline realization. The full
        // policy lives in the `frame_resolution` submodule.
        let frame_outputs = self.resolve_paint_frame_display(
            frame_resolution::FrameResolutionInputs {
                rope_for_projection,
                revision_for_projection,
                decorations,
                caret_bytes_for_projection: &caret_bytes_for_projection,
                folds_for_projection: &folds_for_projection,
                image_reservations: &image_reservations,
                viewport_rows: viewport_rows.clone(),
                display_query: &display_query,
                prewarmed_frame_display: prewarmed_frame_display.take(),
                wrap_width_dip: projection_metrics.wrap_width_dip,
                projection_char_width,
                decoration_parse_revision: current_decoration_parse_revision,
                decoration_parse_advanced,
            },
            &trace,
        );
        let frame_resolution::FrameResolutionOutputs {
            mut frame_display,
            frame_source,
            worker_miss_reason,
            projection_kind,
            current_projection_stamp,
            selection_reveal_dirty,
            should_skip_cache_seed,
            scroll_strip_rows,
        } = frame_outputs;
        if let Some(r) = self.renderer.as_ref() {
            r.set_last_scroll_strip_rows(scroll_strip_rows);
        }
        crate::window_paint_trace::log_projection_stats(
            &frame_display,
            &viewport_rows,
            rope_for_projection.len_lines() as u32,
            frame_source,
            projection_kind.trace_label(),
            worker_miss_reason,
            selection_reveal_dirty.len(),
            image_reservations.len(),
            pane_frame_displays.len(),
            pane_frame_displays,
            pane_table_layouts.iter().map(Vec::len).sum(),
            pane_image_placements.iter().map(Vec::len).sum(),
            spectators.cache_hits,
            spectators.cache_misses,
        );
        self.seed_paint_caches_after_resolve(
            &display_query,
            &frame_display,
            decorations_owned.as_ref(),
            current_decoration_parse_revision,
            should_skip_cache_seed,
        );
        // Hold the caret's source line at the same screen y across an
        // implicit per-paint geometry reflow (a block above the caret
        // revealing / collapsing as the served frame alternates between
        // cache/worker/inline geometries while typing). Runs after the
        // frame is resolved and the row index is cached, so it can shift
        // scroll and cheaply re-realize the corrected viewport with no
        // gap. See `window_view::geometry_anchor`.
        self.apply_geometry_anchor(
            &mut frame_display,
            &display_query,
            rope_for_projection,
            revision_for_projection,
            decorations,
            &caret_bytes_for_projection,
            &folds_for_projection,
            &image_reservations,
            &suppressed_table_blocks,
            projection_metrics.wrap_width_dip,
            projection_char_width,
        );
        let doc_end_snap_action =
            self.apply_pending_doc_end_scroll_after_projection(&frame_display);
        // Drop the hit-test fallback — the just-painted frame
        // supersedes it; the next hover sequence will see
        // `last_painted_frame_display` and reuse without seeding the
        // mouse cache.
        self.mouse_hit_test_frame_cache.borrow_mut().take();
        // Phase C1: build the status-bar segment payload. Owned `Vec`s
        // live in `build` so the `&[…]` borrows inside `status_bar_data`
        // stay valid through `renderer.draw_buffer`.
        let status_bar_build = if self.view_options.show_status_bar {
            Some(self.build_status_bar(
                snap.rope_snapshot().rope(),
                snap.rope_snapshot().revision().get(),
                snap.selections(),
                snap.file.as_ref(),
            ))
        } else {
            None
        };
        trace.mark("status_bar");
        let status_transients = if let Some(build) = status_bar_build.as_ref() {
            let transients = self.status_motion.update(
                &build.segments,
                &build.chips,
                self.motion_policy,
                &mut self.stagger_scheduler,
                motion_now_ms,
            );
            if !transients.is_empty() {
                should_start_motion_timer = true;
            }
            transients
        } else {
            Vec::new()
        };
        let status_bar_data = status_bar_build.as_ref().map(|b| StatusBarData {
            segments: &b.segments,
            chips: &b.chips,
            colors: b.colors,
            transients: &status_transients,
        });
        // Phase G4: build + cache strip layout; project to renderer payload.
        let search_minimap_payload = self.build_search_minimap_payload(
            &body_rect,
            snap.rope_snapshot().rope(),
            search_minimap_active,
            editor_colors,
        );
        self.cache_scaled_minimap_layout(snap.rope_snapshot().rope());
        // Phase C2: cache the layout the painter is about to use so the
        // mouse handler can hit-test against the same x-rects.
        self.view_options.status_bar_layout = status_bar_data.as_ref().map(|d| {
            let top =
                (self.client_height_dip() - continuity_render::STATUS_BAR_HEIGHT_DIP).max(0.0);
            continuity_render::compute_status_bar_layout(
                d,
                self.client_width_dip(),
                top,
                scaled_font_size,
            )
        });
        // Phase F2: build the per-frame outline-sidebar payload + cache
        // its layout for the next mouse hit-test. `outline_entries` must
        // outlive the `OutlineData` it backs because `OutlineData`
        // borrows it as `&[OutlineEntry]`.
        let (outline_entries, outline_colors, outline_current_index) = self
            .build_outline_payload_pieces(
                snap.rope_snapshot().revision().get(),
                snap.rope_snapshot().rope(),
                snap.selections(),
                decorations,
            );
        trace.mark("outline_entries");
        let outline_data = if self.view_options.show_outline_sidebar {
            Some(OutlineData {
                entries: &outline_entries,
                current_index: outline_current_index,
                colors: outline_colors,
                width_dip: self.view_options.outline_sidebar_width_dip,
                font_size_dip: scaled_font_size,
                scroll_offset_dip: self.view_options.outline_scroll_offset_dip,
            })
        } else {
            None
        };
        self.cache_outline_layout(outline_data.as_ref(), body_origin);
        // F5 Pass 2: build the per-frame inline-image placement list
        // from the decoration cache + the focused rope. Empty when
        // `[markdown].inline_images = false` or no `![](url)` spans
        // exist in the focused buffer.
        let image_placements = crate::window_image_placements::build_inline_image_placements(
            decorations,
            snap.rope_snapshot().rope(),
            self.image_store_dir.as_deref(),
            self.inline_images_enabled,
            self.buffer_id,
            &self.image_expand_state,
            &frame_display,
        );
        // `table_layouts` was built above (before the reservations) so
        // its Phase F per-row heights feed the reservation set; it is
        // reused here for the DrawParams payload.
        // Phase I1: build the time-machine HUD payload when the slider
        // overlay is active. Returns `None` when the slider isn't
        // visible, the buffer has no snapshots, or the persist client
        // is missing — all of which leave `time_machine_hud` `None` and
        // skip the renderer's HUD paint pass.
        let time_machine_hud_payload = self.build_time_machine_hud_payload();
        // Build the per-frame view-options + pane-bodies payloads here,
        // *after* all the upstream `&mut self` calls have run. The three
        // borrowed slots in `ViewOptionsDraw` and the `&self.decoration_cache`
        // capture in `pane_bodies` only borrow narrow sub-fields, so they
        // do not block the `&mut self.cache` borrow that
        // `dispatch_renderer_draw` needs below.
        self.apply_deferred_renderer_resize(hwnd)?;
        let file_tree_draw = self.build_file_tree_draw_payload(editor_colors);
        let view_options = self.build_view_options_draw(
            &self.view_options.ruler_columns,
            self.view_options.pane_modes.focus_mode.as_str(),
            &self.view_options.pane_modes.folded_lines,
            scaled_font_size,
            search_minimap_active,
            &heading_lines_for_folds,
        );
        let pane_bodies = crate::window_paint::payload::build_pane_bodies(
            &other_panes,
            &self.decoration_cache,
            pane_table_layouts,
            pane_frame_displays,
            pane_image_placements,
        );
        let (loading_overlay_draw, loading_overlay_motion) =
            self.build_loading_overlay_frame_for_paint(&editor_colors);
        let draw_view = doc_end_snap_action
            .previous_scroll_y_dip
            .map(|scroll_y_dip| {
                let mut view = self.view.clone();
                view.scroll_y_dip = scroll_y_dip;
                view
            });
        let view_ref = draw_view.as_ref().unwrap_or(&self.view);
        let (scroll_target_pane_id, scroll_focused_pane_id, scroll_hover_routed) =
            self.scroll_trace_state();
        let params = DrawParams {
            document,
            format: &format,
            font_state,
            theme_revision: self.active_theme.revision_key(),
            dpi_scale: self.dpi_scale(),
            scroll_velocity_dip_per_s: self.scroll_velocity_dip_per_s(),
            scroll_target_pane_id,
            scroll_focused_pane_id,
            scroll_hover_routed,
            line_height,
            base_font_size_dip: scaled_font_size,
            heading_scale: DEFAULT_HEADING_SCALE,
            view: view_ref,
            colors: editor_colors,
            markdown_colors,
            view_options,
            decorations,
            inline_color_spans: decorations
                .map(|d| d.inline_color_spans.as_slice())
                .unwrap_or(&[]),
            table_overrides: decorations
                .map(|d| d.evaluated_tables.as_slice())
                .unwrap_or(&[]),
            table_layouts: &table_layouts[..],
            overlay: overlay_layer.as_ref().map(|layer| &layer.draw),
            overlay_motion: overlay_layer.as_ref().map(|layer| layer.motion),
            chord_hud: chord_hud_layer.as_ref().map(|layer| &layer.draw),
            chord_hud_motion: chord_hud_layer.as_ref().map(|layer| layer.motion),
            body_origin,
            pane_chrome: chrome.as_ref(),
            spell_spans: &spell_spans,
            pane_bodies: &pane_bodies,
            frame_display: &frame_display,
            line_hover: self.mouse_state.line_hover.map(|hover| LineHoverDraw {
                source_line: hover.source_line,
                display_row: hover.display_row,
                in_gutter: hover.in_gutter,
            }),
            client_height_dip: self.client_height_dip().max(1.0),
            status_bar: status_bar_data.as_ref(),
            file_tree: file_tree_draw.as_ref(),
            jump_glow,
            edit_pulse,
            // Phase F1: breadcrumb paint dispatch is deferred.
            breadcrumb: None,
            // Phase F2: outline-sidebar payload.
            outline: outline_data.as_ref(),
            search_minimap: search_minimap_payload.as_ref(),
            images: Some(&image_placements),
            time_machine_hud: time_machine_hud_payload.as_ref(),
            loading_overlay: loading_overlay_draw.as_ref(),
            loading_overlay_motion,
            code_copy_button: build_code_copy_button_draw(self),
        };
        let metrics_overlay = self.is_metrics_buffer_active();
        let history_overlay = self.has_visible_buffer_history_panes();
        {
            let renderer = self.renderer.as_ref().expect("renderer ready");
            crate::window_paint::dispatch::dispatch_renderer_draw(
                &mut self.cache,
                renderer,
                snap.rope_snapshot().rope(),
                snap.selections(),
                &params,
                metrics_overlay,
                history_overlay,
                &trace,
            )?;
        }
        self.paint_overlay_after_dispatch(metrics_overlay, history_overlay, body_rect)?;
        // ε.5b — dispatch unless a doc-end snap moved the view; then
        // the resolved stamp still names the pre-snap viewport.
        if doc_end_snap_action.previous_scroll_y_dip.is_none() {
            self.submit_projection_worker_request(
                &projection_kind,
                frame_source,
                &current_projection_stamp,
                rope_for_projection,
                decorations_owned.clone(),
                &caret_bytes_for_projection,
                &folds_for_projection,
                &image_reservations,
                projection_char_width,
                &viewport_rows,
            );
        } else if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "event:projection_worker_dispatch",
                "source=doc_end_snap plan=none submitted=false reason=post_snap_repaint",
            );
        }
        let prewarm_realized = frame_display.realized_row_range();
        self.maybe_submit_sliding_scroll_prewarm(prewarm_realized.start, prewarm_realized.end);
        self.finish_paint_epilogue(hwnd, should_start_motion_timer, doc_end_snap_action);
        trace.finish("");
        Ok(())
    }
}

pub(crate) use crate::window_paint_builders::{
    build_pane_chrome, build_spell_squiggle_spans, collect_non_focused_panes, NonFocusedPaneRender,
};
pub(crate) use dispatch::should_skip_projection_worker_request_for_frame_source;
