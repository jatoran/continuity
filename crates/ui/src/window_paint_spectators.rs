//! Per-frame spectator-pane data preparation extracted from
//! [`crate::window_paint`].
//!
//! `on_paint` builds four parallel `Vec`s for the non-focused panes
//! before assembling [`continuity_render::PaneBodyDraw`]: caret byte
//! offsets, pipe-table visual layouts, display projections, and inline-
//! image placements. Each entry lines up with the same index into the
//! `&[NonFocusedPaneRender]` slice. Bundling the build into one helper
//! keeps the orchestrator under the conventions cap and keeps the
//! parallel-index invariant in one place.
//!
//! Thread ownership: UI thread of one window. Reads `Window`'s
//! decoration cache, image-store state, and pane snapshots; mutates
//! only the per-pane spectator `FrameDisplay` cache.
//!
//! Spectator projection cost is bounded by
//! [`crate::window_spectator_cache::SpectatorFrameCache`]: when the
//! pane's rope / decoration / fold / wrap / font geometry has not
//! shifted since the last paint, the previous frame is cloned
//! verbatim instead of cold-built. The cache key intentionally
//! ignores spectator caret bytes — non-focused panes rarely change
//! selection mid-typing-in-the-focused-pane and the eventual stale
//! reveal of a moved-then-re-rendered marker is one paint behind,
//! not visually wrong.

use std::path::Path;

use continuity_display_map::FoldRange;
use continuity_render::{FrameDisplay, InlineImagePlacement};

use crate::display_prewarm_cache::PrewarmQuery;
use crate::window::{Window, LINE_HEIGHT_DIP};
use crate::window_paint::{visible_display_row_range, VIEWPORT_OVERSCAN_ROWS};
use crate::window_paint_builders::NonFocusedPaneRender;
use crate::window_projection_plan::realized_covers;
use crate::window_projection_worker::{current_projection_stamp, PaintProjectionInputs};
use crate::window_spectator_cache::SpectatorCacheLookup;

/// Emit a `paint:spectator_cache` trace line. `action` is the
/// `hit=…[ miss=…]` prefix; `fields` carries the remaining
/// `key=value` payload. No-op when tracing is disabled, so callers do
/// not need to wrap every arm in `is_trace_enabled` themselves.
pub(super) fn log_spectator_cache(pane_idx: usize, action: &str, fields: &str) {
    if !crate::paint_trace::is_trace_enabled() {
        return;
    }
    crate::paint_trace::log_event(
        "paint:spectator_cache",
        &format!("pane={} {} {}", pane_idx, action, fields),
    );
}

fn max_scroll_y_for_frame(frame_display: &FrameDisplay, viewport_height_dip: f32) -> f32 {
    let content_h = frame_display.display_line_count().max(1) as f32 * LINE_HEIGHT_DIP;
    (content_h - viewport_height_dip.max(0.0)).max(0.0)
}

fn should_clamp_scroll_y_to_frame(
    frame_display: &FrameDisplay,
    scroll_y_dip: f32,
    viewport_height_dip: f32,
) -> bool {
    if !scroll_y_dip.is_finite() || scroll_y_dip < 0.0 {
        return true;
    }
    let total_display_rows = frame_display.display_line_count();
    let visible_rows =
        visible_display_row_range(scroll_y_dip, viewport_height_dip, LINE_HEIGHT_DIP);
    total_display_rows == 0 || visible_rows.start >= total_display_rows
}

fn clamp_scroll_y_to_frame(
    frame_display: &FrameDisplay,
    scroll_y_dip: f32,
    viewport_height_dip: f32,
) -> f32 {
    if scroll_y_dip.is_finite() {
        scroll_y_dip.clamp(
            0.0,
            max_scroll_y_for_frame(frame_display, viewport_height_dip),
        )
    } else {
        0.0
    }
}

fn frame_paint_row_range(
    frame_display: &FrameDisplay,
    scroll_y_dip: f32,
    viewport_height_dip: f32,
) -> std::ops::Range<u32> {
    let total_display_rows = frame_display.display_line_count();
    let visible_rows =
        visible_display_row_range(scroll_y_dip, viewport_height_dip, LINE_HEIGHT_DIP);
    let start = visible_rows.start.min(total_display_rows);
    let end = visible_rows.end.min(total_display_rows).max(start);
    start..end
}

/// Per-frame bundle of spectator-pane data. Each `Vec` is indexed by
/// the same position in the parent `&[NonFocusedPaneRender]` slice.
///
/// Per-spectator caret byte offsets live as a local inside
/// [`build_spectator_pane_data`] — they feed the three published `Vec`s
/// during the build and are not needed downstream.
pub(crate) struct SpectatorPaneData {
    /// Pipe-table visual layouts per spectator. Empty when the pane's
    /// buffer has no `evaluated_tables` or when decorations are cold.
    pub table_layouts: Vec<Vec<continuity_render::TableLayout>>,
    /// Display projection (hide / replace / soft-wrap / fold) per
    /// spectator. Built fresh each frame from the pane's rope + caret
    /// bytes — or reused from
    /// [`crate::window_spectator_cache::SpectatorFrameCache`] when the
    /// pane's projection geometry has not changed.
    pub frame_displays: Vec<FrameDisplay>,
    /// Inline-image placements per spectator. Empty when the buffer
    /// has no `![](url)` spans or `[markdown].inline_images` is off.
    pub image_placements: Vec<Vec<InlineImagePlacement>>,
    /// Cache hits across all spectators this frame.
    pub cache_hits: u32,
    /// Cache misses (stale or empty) across all spectators this frame.
    pub cache_misses: u32,
}

/// Very large no-wrap spectator panes use the documented stub path:
/// cache by no-wrap geometry, paint the last compatible projection
/// when available, and pay the row-count walk only on the first miss.
const SPECTATOR_STUB_LINE_THRESHOLD: usize = 2_000;

/// Build [`SpectatorPaneData`] for every non-focused pane in one pass.
/// `projection_char_width` is the focused-pane DIP advance used as the
/// fallback monospace measure for table column widths and soft-wrap.
/// The actual soft-wrap column is resolved per spectator from that
/// pane's current body rect so an unfocused wrapped pane paints on the
/// same display-row grid it projected.
///
/// `cached_image_dimensions` peeks the renderer's image cache for the
/// γ row-reservation provider; spectator panes with no expanded image
/// or cold-cache paths fall through to the pre-reservation single-row
/// projection.
pub(crate) fn build_spectator_pane_data(
    window: &Window,
    other_panes: &mut [NonFocusedPaneRender],
    projection_char_width: f32,
    cached_image_dimensions: &mut dyn FnMut(&Path) -> Option<(u32, u32)>,
) -> SpectatorPaneData {
    // Retain entries for **every** pane in the tree, not just the
    // current non-focused set. The just-focused pane (now absent from
    // `other_panes`) still needs its prior spectator frame so
    // `Window::resolve_paint_frame_display`'s spectator-promote can
    // skip a 400 ms cold build on the very first paint after a focus
    // switch. The cache is bounded by the pane count anyway —
    // entries for collapsed / closed panes are correctly dropped
    // because they are absent from the tree.
    let live_panes: Vec<crate::pane_tree::PaneId> =
        window.tree.root.leaf_ids().into_iter().collect();
    window
        .spectator_frame_cache
        .borrow_mut()
        .retain_panes(&live_panes);

    let caret_bytes: Vec<Vec<usize>> = other_panes
        .iter()
        .map(|p| {
            let rope = p.snapshot.rope_snapshot().rope();
            p.snapshot
                .selections()
                .iter()
                .map(|s| {
                    let line = s.head.line as usize;
                    let line_start = if line < rope.len_lines() {
                        rope.line_to_byte(line)
                    } else {
                        rope.len_bytes()
                    };
                    line_start + s.head.byte_in_line as usize
                })
                .collect()
        })
        .collect();
    let table_layouts: Vec<Vec<continuity_render::TableLayout>> = other_panes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let Some(dec) = window.decoration_cache.get(p.document) else {
                return Vec::new();
            };
            if dec.evaluated_tables.is_empty() {
                return Vec::new();
            }
            // Skip the table layout build when decorations lag the
            // rope. The renderer paints the source bytes raw for the
            // one frame between an edit and the worker delivering an
            // updated parse — better than panicking in `byte_slice`
            // on a stale `block_range` that crosses a multi-byte
            // char (see `crates/render/src/table_layout/build.rs`
            // for the in-builder defensive guard).
            let rope = p.snapshot.rope_snapshot().rope();
            let revision = p.snapshot.rope_snapshot().revision().0;
            if dec.revision != revision {
                return Vec::new();
            }
            let mut measure = |text: &str| text.chars().count() as f32 * projection_char_width;
            // Compute per-pane suppression from this pane's own
            // selection — a Ctrl+A in pane A must not unrender the
            // same buffer's table in pane B.
            let suppressed = continuity_render::compute_suppressed_table_blocks(
                rope,
                p.snapshot.selections(),
                &dec.evaluated_tables,
            );
            continuity_render::compute_table_layouts(
                &dec.evaluated_tables,
                rope,
                &caret_bytes[i],
                &suppressed,
                &mut measure,
            )
        })
        .collect();
    let mut frame_displays: Vec<FrameDisplay> = Vec::with_capacity(other_panes.len());
    let mut cache_hits: u32 = 0;
    let mut cache_misses: u32 = 0;
    for (i, p) in other_panes.iter_mut().enumerate() {
        let dec = window.decoration_cache.get(p.document);
        let rope = p.snapshot.rope_snapshot().rope();
        let revision = p.snapshot.rope_snapshot().revision().0;
        // γ / Phase F — same reservation pass as the focused pane,
        // scoped to this spectator's buffer + body width, merged with
        // this pane's table-row reservations so `<br>` / wrapped table
        // rows reserve their extra display rows here too.
        let reservations = crate::window_image_placements::compute_image_reservations_for_pane(
            dec,
            rope,
            window.image_store_dir.as_deref(),
            window.inline_images_enabled,
            p.buffer_id,
            &window.image_expand_state,
            cached_image_dimensions,
            LINE_HEIGHT_DIP,
            p.rect.2.max(1.0),
        );
        let reservations = crate::window_image_placements::merge_table_row_reservations(
            reservations,
            &table_layouts[i],
        );
        let wrap_width_dip = if p.view.soft_wrap {
            continuity_render::pane_body::spectator_body_text_width_with_right_edge_for_line_count_dip(
                p.rect.2,
                window.scaled_font_size(),
                window.view_options.line_numbers,
                rope.len_lines(),
                p.minimap,
                p.show_outline_sidebar,
                window.view_options.outline_sidebar_width_dip,
            )
            .round()
            .max(0.0) as u32
        } else {
            0
        };
        // γ — the spectator's `PrewarmQuery` carries this pane's
        // image-row-reservation signature, so a buffer with expanded
        // inline images (or, later, multi-line table rows) hits the
        // cache as long as the reservation set is stable. A change in
        // the set bumps the signature and misses, forcing exactly one
        // rebuild. Previously a non-empty set forced an unconditional
        // `Stale`, cold-walking the whole document every paint — the
        // multi-pane paint cliff that reverted Phase F twice.
        let query = PrewarmQuery::new(
            p.buffer_id,
            revision,
            dec.map(|d| d.revision),
            &caret_bytes[i],
            &[],
            wrap_width_dip,
            window.font_state,
        )
        .with_image_reservations(&reservations);
        let spectator_folds: [FoldRange; 0] = [];
        let cache_outcome = window
            .spectator_frame_cache
            .borrow_mut()
            .lookup(p.pane_id, &query);
        // Spectator cold builds are bounded by the visible row range
        // plus overscan instead of the full document. Building the
        // whole-document `DisplayRowIndex` via the cheap row-count
        // walker still happens (offscreen scrollbar / EOF / hit-test
        // lookups need it), but spec materialization runs only for
        // rows the spectator actually paints. On a 9 k-line markdown
        // buffer in a non-focused pane this drops the first-paint
        // cold build from ~250 ms to ~visible-row cost — the worst
        // case the manual trace surfaces is a brand-new split into a
        // large buffer where two cold spectators paid 531 ms total.
        // The spectator's body rect is the source of truth for
        // viewport height because the per-pane `view.viewport_height_dip`
        // is updated only when the pane is focused.
        let spectator_viewport_h = p.rect.3.max(0.0);
        p.view.viewport_height_dip = spectator_viewport_h;
        p.view.viewport_width_dip = p.rect.2.max(0.0);
        let visible_rows =
            visible_display_row_range(p.view.scroll_y_dip, spectator_viewport_h, LINE_HEIGHT_DIP);
        let projection_stamp = current_projection_stamp(&PaintProjectionInputs {
            buffer_id: p.buffer_id,
            rope_revision: revision,
            decoration_revision: dec.map(|d| d.revision),
            decoration_parse_revision: dec.map(|d| d.revision),
            caret_bytes: &caret_bytes[i],
            folds: &spectator_folds,
            image_reservations: &reservations,
            wrap_width_dip,
            // Deferred font-swap (see `window_font_swap`): stamp with
            // the pending target font_state so spectator panes rebuild
            // in step with the focused pane.
            font_state: window.effective_font_state(),
            viewport_rows: visible_rows.clone(),
            overscan: VIEWPORT_OVERSCAN_ROWS,
        });
        let stub_eligible = !p.view.soft_wrap
            && reservations.is_empty()
            && rope.len_lines() > SPECTATOR_STUB_LINE_THRESHOLD;
        let large_spectator_partial_eligible = rope.len_lines() > SPECTATOR_STUB_LINE_THRESHOLD;
        // γ — reservation-bearing spectators now seed too: the query
        // carries the reservation signature, so the seeded frame is
        // reused only by a paint with the identical reservation
        // geometry. Without seeding, the steady-state hit that fixes
        // the multi-pane cliff could never land. Large stale misses,
        // including reservation-signature drift, stay on the bounded
        // current-geometry partial path so a 2x2 grid cannot run a
        // full spectator row-count walk on the UI thread.
        let mut should_seed_spectator_cache = !stub_eligible;
        // Pending-worker gate. Computed once and reused across both
        // branches so a pending worker submission at the current stamp
        // suppresses the full cold walker regardless of whether the
        // lookup returned `Stale(_)` or `Empty`. Stale/empty misses now
        // paint a current-geometry partial frame instead of an
        // unrealized placeholder; the worker/full fill is still
        // responsible for replacing those partial row indexes after
        // paint. Same-geometry realized-range misses are different:
        // their partial extends a compatible cached frame and is safe
        // to seed immediately.
        let worker_pending = window.has_pending_spectator_projection(p.pane_id, &projection_stamp);
        let mut frame_display = if stub_eligible {
            let stub_query = PrewarmQuery::new(
                p.buffer_id,
                revision,
                dec.map(|d| d.revision),
                &caret_bytes[i],
                &[],
                0,
                window.font_state,
            );
            match window
                .spectator_frame_cache
                .borrow_mut()
                .lookup(p.pane_id, &stub_query)
            {
                SpectatorCacheLookup::Hit(fd) => {
                    cache_hits += 1;
                    log_spectator_cache(
                        i,
                        "hit=true",
                        &format!("cache=stub_no_wrap source_lines={}", rope.len_lines()),
                    );
                    fd
                }
                SpectatorCacheLookup::Stale(_) | SpectatorCacheLookup::Empty => {
                    cache_misses += 1;
                    log_spectator_cache(
                        i,
                        "hit=false miss=stub_no_wrap",
                        &format!(
                            "source_lines={} viewport={}..{}",
                            rope.len_lines(),
                            visible_rows.start,
                            visible_rows.end,
                        ),
                    );
                    let built = window.build_frame_display_viewport_cached(
                        Some(p.buffer_id),
                        rope,
                        revision,
                        dec,
                        &caret_bytes[i],
                        &[],
                        &reservations,
                        0,
                        projection_char_width.max(1.0),
                        visible_rows.clone(),
                        VIEWPORT_OVERSCAN_ROWS,
                        continuity_display_map::WalkerCallReason::PaintCold,
                    );
                    let dec_rev = dec.map(|d| d.revision);
                    let dec_arc = dec.cloned().map(std::sync::Arc::new);
                    window.spectator_frame_cache.borrow_mut().insert(
                        p.pane_id,
                        stub_query,
                        built.clone(),
                        dec_arc,
                        dec_rev,
                    );
                    built
                }
            }
        } else {
            let resolve = cache_resolve::resolve_main_cache_outcome(
                cache_resolve::SpectatorCacheResolveInputs {
                    window,
                    pane_idx: i,
                    pane_id: p.pane_id,
                    buffer_id: p.buffer_id,
                    rope,
                    revision,
                    decorations: dec,
                    caret_bytes: &caret_bytes[i],
                    reservations: &reservations,
                    wrap_width_dip,
                    projection_char_width,
                    visible_rows: visible_rows.clone(),
                    query: &query,
                    large_spectator_partial_eligible,
                    worker_pending,
                },
                cache_outcome,
            );
            cache_hits += resolve.cache_hit_delta;
            cache_misses += resolve.cache_miss_delta;
            if !resolve.seed_after_paint {
                should_seed_spectator_cache = false;
            }
            resolve.frame_display
        };
        if should_clamp_scroll_y_to_frame(&frame_display, p.view.scroll_y_dip, spectator_viewport_h)
        {
            let clamped_scroll_y_dip =
                clamp_scroll_y_to_frame(&frame_display, p.view.scroll_y_dip, spectator_viewport_h);
            let clamped_visible_rows = visible_display_row_range(
                clamped_scroll_y_dip,
                spectator_viewport_h,
                LINE_HEIGHT_DIP,
            );
            let clamped_paint_rows =
                frame_paint_row_range(&frame_display, clamped_scroll_y_dip, spectator_viewport_h);
            let realized = frame_display.realized_row_range();
            if !realized_covers(realized.clone(), &clamped_paint_rows) {
                frame_display = realtime_miss::build_spectator_viewport_partial(
                    window,
                    rope,
                    revision,
                    dec,
                    &caret_bytes[i],
                    &spectator_folds,
                    &reservations,
                    wrap_width_dip,
                    projection_char_width.max(1.0),
                    clamped_visible_rows.clone(),
                    Some(&frame_display),
                );
                should_seed_spectator_cache = false;
            }
            if crate::paint_trace::is_trace_enabled() {
                crate::paint_trace::log_event(
                    "paint:spectator_scroll_clamp",
                    &format!(
                        "pane={} scroll_y={:.2}->{:.2} viewport={}..{}",
                        i,
                        p.view.scroll_y_dip,
                        clamped_scroll_y_dip,
                        clamped_visible_rows.start,
                        clamped_visible_rows.end,
                    ),
                );
            }
            p.view.scroll_y_dip = clamped_scroll_y_dip;
        }
        if should_seed_spectator_cache {
            // Cache decorations + revision alongside the frame so a
            // later focus switch's spectator-promote can install them
            // as `last_painted_decorations` and run a tight
            // `diff_dirty_lines` instead of falling to Cold.
            let dec_rev = dec.map(|d| d.revision);
            let dec_arc = dec.cloned().map(std::sync::Arc::new);
            window.spectator_frame_cache.borrow_mut().insert(
                p.pane_id,
                query,
                frame_display.clone(),
                dec_arc,
                dec_rev,
            );
        }
        frame_displays.push(frame_display);
    }
    let image_placements: Vec<Vec<InlineImagePlacement>> = other_panes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let dec = window.decoration_cache.get(p.document);
            let rope = p.snapshot.rope_snapshot().rope();
            crate::window_image_placements::build_inline_image_placements(
                dec,
                rope,
                window.image_store_dir.as_deref(),
                window.inline_images_enabled,
                p.buffer_id,
                &window.image_expand_state,
                &frame_displays[i],
            )
        })
        .collect();
    SpectatorPaneData {
        table_layouts,
        frame_displays,
        image_placements,
        cache_hits,
        cache_misses,
    }
}

pub(crate) mod cache_resolve;
pub(crate) mod pending_worker_gate;
pub(crate) mod realtime_miss;
