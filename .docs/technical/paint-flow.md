# Paint frame flow

Walkthrough of `WM_PAINT` for one frame. Every visible pane body, chrome strip, overlay, and the global status bar are drawn here.

## Entry point

`crates/ui/src/window.rs::wndproc` routes `WM_PAINT` to
`Window::on_paint(hwnd)` in `crates/ui/src/window_paint.rs`. The
paint-tail Win32 validation, caret-blink arm, motion timer arm, and
post-`EndPaint` document-end repaint live in
`crates/ui/src/window_paint/epilogue.rs`.

## Resize lifecycle

`WM_SIZE` does **not** rebuild the renderer. `Window::refresh_client_size`
uses `Renderer::resize_for_hwnd(hwnd, w, h)` (in
`crates/render/src/renderer/resize.rs`) when the target rebind is immediate;
that rebinds only the swap-chain back buffer + D2D target bitmap at the HWND's live DPI
and re-issues `ID2D1DeviceContext::SetDpi(dpi, dpi)` so the device context's
DIP→pixel drawing transform tracks the live DPI (the target bitmap's `dpiX/dpiY`
only sets the DIP size the bitmap reports, not the drawing transform — without
the explicit `SetDpi` the DIP-native layout would paint at 96 DPI and leave a
right/bottom band at >100 % scale). The
D3D11/D2D/DirectWrite stack and the image cache are reused.
`WM_DPICHANGED` uses the same target-rebind path after applying Windows'
suggested rect. A full-client `InvalidateRect(hwnd, None, false)` is issued
on every real size delta so shrink motions repaint (the window class lacks
`CS_HREDRAW`/`CS_VREDRAW` and `DefWindowProc` only invalidates exposed
regions on grow). The main top-level HWND is created with
`WS_EX_NOREDIRECTIONBITMAP` because D2D/DXGI owns the full client visual; this
keeps DWM from exposing a stale redirection surface at the right/bottom edge
during live shrink. Between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`, the
focused-pane viewport is updated via the unanchored fast path and the
caret-line screen-y anchor runs exactly once at drag release. During that
live sizing loop, logical client/layout size updates on every tick. Grow ticks
resize the swap-chain target immediately, while shrink ticks that still fit
inside the current target defer the target rebind until `Window::paint` has
built the replacement frame and is about to call the renderer. That keeps the
old presented frame available during projection work and narrows the invalid
resized-target interval to the draw/present section. The HWND swap chain uses
`DXGI_SCALING_STRETCH` so DXGI can stretch the last presented frame across the
short interval between `WM_SIZE` and the next paint. The window procedure also consumes
`WM_ERASEBKGND` and returns handled because the D2D/DXGI paint path owns the
full-client clear; default background erasure during modal sizing can expose a
transient right/bottom edge. While live resizing, `WM_SIZE` calls
`UpdateWindow` after the full-client invalidate so the pending repaint is sent
before control returns to the sizing loop. On shrink-axis ticks, the handler
then calls `DwmFlush` so the newly painted frame is consumed by the compositor.
The cold
`Renderer::for_hwnd` path is taken only on first paint or after
`resize_for_hwnd()` returns an error.
Resize also drops the retained static-chrome command list and the
per-table chrome cache; the next paint re-records both against the
fresh D2D target.

## Step-by-step

### 1. Snapshot
```rs
let snap: EditorSnapshot = self.current_snapshot()?;        // Arc<Rope> + selections + file
let rope = snap.rope_snapshot().rope();
let decorations: Option<&Decorations> = self.decoration_cache.get(self.buffer_id);
```

### 2. Per-pane data
```rs
let other_panes = collect_non_focused_panes(self);
let pane_bodies: Vec<PaneBodyDraw> = build_pane_bodies(&other_panes, ...);
// each spectator carries: buffer_id, snapshot, view, rect,
// minimap flag, outline flag, focused=false
```

### 3. Resolve `FrameDisplay`
```rs
let caret_bytes: Vec<usize> = snap.selections.iter().map(…).collect();
let projection_wrap_width: u32 = if self.view.soft_wrap {
    (self.view.viewport_width_dip - GUTTER_WIDTH_DIP).max(0.0).round() as u32
} else { 0 };

let frame_display = self.resolve_paint_frame_display(...).frame_display;
```

`resolve_paint_frame_display` computes the projection stamp, classifies the build (`CacheHit`, `SelectionRebuild`, `Dirty`, `Splice`, `Cold`, or a P18 partial variant), polls the projection worker once, and immediately falls through to the inline/cache/partial realization when the worker missed. It never blocks the UI thread waiting for the worker. `FrameDisplay` contains an `Arc<DisplayMap>` with realized display rows plus the row index needed for offscreen queries. A `CacheHit` whose frame row index is partial still paints immediately, then `window_paint/dispatch.rs` submits a background `ProjectionPlan::Cold` with reason `paint_partial_fill` so a full row index is available for later focus, scroll, caret, and hit-test paths. See [`display-map.md`](../design/features/display-map.md).

### 4. Construct `DrawParams`
```rs
let params = DrawParams {
    document:    Some(&document_chrome),
    format:      &self.text_format,
    font_state:  self.font_state,
    theme_revision: self.active_theme.revision_key(),
    dpi_scale:   self.dpi_scale(),
    line_height: LINE_HEIGHT_DIP,
    base_font_size_dip: scaled_font_size,
    heading_scale: DEFAULT_HEADING_SCALE,
    view:        view_ref,
    colors:      self.active_theme.editor_colors(),
    markdown_colors: self.active_theme.markdown_colors(),
    view_options,
    decorations,
    overlay:     overlay_draw.as_ref(),
    overlay_motion,
    chord_hud:   chord_hud_draw.as_ref(),
    chord_hud_motion,
    jump_glow,
    body_origin,
    pane_chrome: chrome.as_ref(),
    spell_spans: &spell_spans,
    pane_bodies: &pane_bodies,
    frame_display: &frame_display,
    file_tree: file_tree_draw.as_ref(),
    client_height_dip: self.client_height.max(1) as f32,
};
```

`view_options` is `ViewOptionsDraw` — the renderer-facing view of `Window::view_options` (the per-pane `ViewOptions` struct). Motion fields are pre-projected on the UI thread; render does not own clocks or timers.
Focused-pane minimap / outline visibility comes from `view_options`;
spectator-pane minimap / outline visibility comes from each
`PaneBodyDraw`, which was resolved from the active buffer's
right-edge chrome state during `collect_non_focused_panes`.

### 5. Renderer.draw_buffer
`crates/render/src/renderer.rs::Renderer::draw_buffer(rope, selections, &mut cache, &params)`:

```rs
unsafe {
    self.d2d_context.BeginDraw();
    self.d2d_context.Clear(Some(&bg_d2d));

    // Phase 13: translate body to focused-pane origin
    let body_translate = Matrix3x2 { …M31: params.body_origin.0, M32: params.body_origin.1 };

    // Build brushes from theme colors.
    let caret_brush   = brushes.solid(params.colors.cursor_primary)?;
    let bg_brush      = brushes.solid(params.colors.background)?;
    let fg_brush      = brushes.solid(params.colors.foreground)?;
    let line_highlight_brush = brushes.solid(params.colors.line_highlight)?;
    let line_number_brush    = brushes.solid(params.colors.line_number)?;
    // …

    if params.view_options.current_line_highlight {
        // B16: spans every display row of the caret's source line
        let display_rows = selections.first().map(|s| {
            let l = s.head.line as usize;
            (frame_display.first_display_line_index_for_source(l),
             frame_display.display_line_count_for_source(l))
        });
        paint_current_line_highlight(&self.d2d_context, rope, selections,
            line_height, scroll_y, viewport_w, margins, &line_highlight_brush, display_rows);
    }

    if params.view_options.trailing_whitespace { paint_trailing_whitespace(…); }

    let use_wrap_paint = params.view.soft_wrap;
    if use_wrap_paint {
        crate::wrap_paint::paint_display_lines(…)?;
    } else {
        for line_idx in first_visible..last_visible {
            // 1. SetTransform to layout-local
            // 2. LayoutCache::get_or_build keyed by (buffer, line, revision, font_state, wrap)
            // 3. Draw selection rects under text
            // 4. DrawTextLayout
            // 5. Draw caret rects (caret_rect_for_shape — B4-aware bar width)
            // 6. Apply structural-skip if caret landed inside a hidden range
        }
    }

    // P14.1: replay one ID2D1CommandList per visible focused-pane table.
    // The lists were recorded into table_chrome_cache before BeginDraw via
    // renderer_table_chrome::prepare_retained_chrome; here we install each
    // table's screen-space transform and DrawImage the cached list, then
    // restore the body transform.
    crate::renderer_table_chrome::run_replay(self, &mut table_chrome_plan, …)?;

    crate::spell::paint_spell_spans(&self.d2d_context, &render_target, cache, params, …)?;
    paint_indent_guides(…); paint_line_number_gutter(…); paint_minimap(…);
    crate::pane_body::paint_all_pane_bodies(&self.d2d_context, &self.dwrite_factory, cache, params, line_height, &fg_brush)?;
    if let Some(glow) = params.jump_glow { paint_jump_glow(…); }
    crate::pane_chrome::paint_pane_chrome(…)?;
    retained_chrome_command_list.DrawImage(…); // ruler columns + static shells

    if params.view_options.show_status_bar {
        crate::status_bar::paint_status_bar_frame_text(…); // text only; shell retained
    }
    if let Some(file_tree) = params.file_tree {
        crate::file_tree_paint::paint_file_tree(…);
    }
    if let Some(overlay) = params.overlay { paint_overlay_with_motion(…); }
    if let Some(chord_hud) = params.chord_hud { paint_overlay_with_motion(…); }

    self.d2d_context.EndDraw(None, None)?;
    self.swap_chain.Present(1, 0)?;
}
```

## Layout cache lookup

`crates/layout/src/cache.rs::LayoutCache::get_or_build(key, dwrite, layout_text, format, font_size, line_height, soft_wrap_width, syntax_highlights, markdown_colors, heading_scale) -> &CacheEntry`:

Key = `(buffer_id, line_idx, revision_built, font_state, soft_wrap_width)`.
`font_state` includes the per-window DPI scale, so monitor moves
naturally miss old-DPI layouts. LRU-bounded at ~10× visible lines per
pane. On miss:
1. Create a fresh `IDWriteTextLayout` from `layout_text` (which is `DisplayLineSpec::display_text()`).
2. Apply `style_runs` (block style + inline style overlays).
3. Apply syntax highlights for code blocks.
4. Apply font scale for headings (from `markdown_colors` + `heading_scale`).
5. Store the layout + content_stamp.

Manual trace note: `CONTINUITY_UI_TRACE=<path>` now snapshots
`LayoutCacheCounters` around `Renderer::draw_buffer*` and emits one
`paint:render_stats` line per paint. The line reports layout cache
hits/misses, layouts created, focused display rows drawn, source lines
visited, soft-wrap continuations, spectator rows, and feature counts
(spell/table/image/minimap/outline/status).
`event:chrome_path mode=fresh|replay elapsed_us=N` is emitted beside
those rows and reports whether the retained static-chrome command list
was rebuilt for this paint or replayed from cache.
`event:table_chrome_path mode=fresh|replay tables=N fresh=N replay=N
record_us=N replay_us=N elapsed_us=N` is the per-table P14.1
counterpart — `tables` is the count of visible focused-pane tables this
paint, `fresh` / `replay` split them by record vs replay, and the same
total appears as `chrome_overlay_table_us` inside
`event:renderer_draw_stages`.

Trace files start with `trace_columns` and `trace_open` metadata rows
(schema, sink, flush mode, process/build/target, cwd, argv). During a normal
paint, `paint:window_state` records the focused snapshot revision/line count,
primary caret line/byte, pane/tab ids, active buffer ids, view scroll/viewport,
overlay/focus/minimized flags, last-frame rows, and spectator-cache counters.
If paint cannot obtain a core snapshot for `Window::buffer_id`, it emits
`paint:no_snapshot` with the same state plus `missing_buffer_tabs` and the
first missing tab/buffer id before presenting the clear frame. WndProc timing
rows also carry `ctx_buffer`, `ctx_pane`, `ctx_tab`, and `ctx_kind` so stalls
can be tied back to the visible tab without correlating against later paint
rows.

## Wrap paint

`crates/render/src/wrap_paint.rs::paint_display_lines` walks focused-pane `DisplayLineSpec`s in display-row order; `crates/render/src/pane_body.rs` does the same for non-focused pane bodies. Each `DisplayLineSpec` produces one `IDWriteTextLayout`. Caret painting uses `display_byte_to_utf16` for hit-test conversion. Same `caret_rect_for_shape` is used; selection rects use `source_byte_in_line_to_display_utf16` to convert source-byte selection ranges into display utf16 coords. F3 inline-color and F4 table-formula overlays run against each concrete display spec so restored word-wrap sessions paint the same extensions as the non-wrap path.

## Projection worker latency

Each `ProjectionResult` carries `build_dur_us: u64` (wall time the worker thread spent inside `build_for_request`) and `coalesced_dropped: u32` (number of queued requests this build replaced via latest-wins). Paint surfaces both on the worker_hit arm and on stale-result rejections so a trace can attribute "the worker is too slow" vs "the UI thread is producing requests faster than the worker can build":

- `paint:frame_display:worker_hit seq=… viewport=…..… build_dur_us=… coalesced_dropped=…` — accepted result.
- `event:projection_worker_result seq=… accepted=true build_dur_us=… coalesced_dropped=…` — same data, indexed by seq for cross-paint correlation.
- `event:projection_worker_stale_result seq=… field=… build_dur_us=… coalesced_dropped=… stale_rope_rev=… paint_rope_rev=… stale_decoration_rev=… paint_decoration_rev=…` — worker produced a result but its stamp drifted past the current paint. Field names the first stamp slot that mismatched (`rope_revision` / `caret_signature` / `viewport` / `decoration_revision` / `wrap_width_dip` / `fold_signature` / `font_state` / `image_reservations_signature`). The stale-rev / paint-rev pair shows how far behind the worker was.
- `event:worker_hit_stages extract_us=… event_log_us=… paint_marks_us=… arm_total_us=… seq=…` — worker-hit install arm sub-stages. The arm should stay microseconds-scoped; a large `frame_display:worker_hit` paint mark means time was spent before the hit arm, not moving the `FrameDisplay`.

Early dispatch (`window_projection_early_dispatch.rs::try_dispatch_projection_worker_early`) is hooked from the edit-completion paths (`insert_text`, `delete_back`, `delete_forward`, `selection_edit`) and from non-edit transitions such as `switch_focus`, `adopt_buffer_as_new_tab`, `reopen_closed_tab`, pane split/resize/maximize, file open, and committed `WM_SIZE`. The call-site `reason=` stays fine-grained on `event:projection_worker_early_dispatch`; `event:projection_worker_queue_depth reason=…` uses the coarse submit category (`early_dispatch`, `layout_change`, `focus_change`, `paint_epilogue`, `paint_partial_fill`). Focused-pane worker requests carry per-paint image-row reservations, and acceptance validates their signature in the projection stamp. Submitting at the transition gives the worker a head start before the next paint without making paint wait for completion.

Focus-change early dispatch has one special prewarm policy in
`window_projection_early_dispatch/focus_prewarm.rs`: when the newly
focused pane can paint from a partial cache hit, or classify returns
`ColdPartial` / `DirtyPartial` / `SplicePartial`, the worker receives a
`ProjectionPlan::Cold` under `submit_reason=focus_change`. The UI paints
the bounded partial immediately; the worker fills the full row index
off-thread before the next navigation / hit-test path needs it. Identical
stamps still dedupe through `Window::last_early_dispatch_stamp`.

The projection-worker receive loop keeps latest-wins per target pane, with
one exception for partial fills. If an older `paint_partial_fill` request has
the same document, rope revision, font state, and wrap width as a newer
non-fill request for the same pane, both are retained so the full-index fill
is not starved by epilogue churn. A newer partial fill replaces an older
partial fill, and any stamp drift drops the older fill as stale.

### Smooth-scroll paint path (P12)

Wheel input adds impulses to a UI-thread-owned inertia accumulator
(`crates/ui/src/window_scroll.rs`). `[editor].mouse_wheel_scroll_speed`
multiplies the base 3-line notch distance before the impulse is queued;
the default `2.0` yields 6 lines/notch. The shared scroll animation timer
(~60 Hz) advances `ViewState.scroll_y_dip` fractionally; paint
translates the body by that value, so glyphs render at fractional
sub-pixel positions during inertia.

A scroll-anim paint that reuses a compatible cached frame picks one
of three actions in `window_paint/frame_resolution/scroll_anim_action.rs`:

| Uncovered rows in viewport | Action | Trace |
|---|---|---|
| `0` (cached realized covers live viewport) | reuse the cached frame as-is | `paint:frame_display:scroll_anim_reuse` |
| `1..=80` (`SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS`) | extend the realized window via `rebuild_frame_display_dirty` for the strip rows; seed the cache | `paint:frame_display:scroll_anim_strip_realize rows=N elapsed_us=…` |
| `> 80` | reuse prev frame; the renderer's placeholder pass fills unrealized rows; don't seed the cache | `paint:frame_display:scroll_anim_placeholder rows=N` |

Once per paint, while inertia is active, `Window::maybe_submit_sliding_scroll_prewarm`
checks the realized lead vs `0.5 × viewport_height` in the direction
of velocity and submits one worker request for the next-page-ahead
viewport via the existing early-dispatch path
(`submit_reason=scroll_prewarm`). The worker's latest-wins channel
coalesces duplicates.

`event:scroll_path` summarises the paint:
`mode={cold|fractional_only|fractional_realized|fractional_placeholder}
elapsed_us=… velocity_dip_per_s=… rows_realized_synchronously=N
rows_placeholder=N scroll_y_dip=… frame_scroll_y_dip=…`. Mode
precedence: `fractional_placeholder` > `fractional_realized` >
`fractional_only` > `cold`.

The body translation arithmetic is always driven by the live
`params.view.scroll_y_dip`, not the cached frame's stamp — the regression
that produced flat blank rows during inertia was the painter skipping
unrealized rows, not the offset math.

Scroll bounds for wheel and scrollbar paths are computed from the current
frame's concrete `FrameDisplay::display_line_count() * line_height`.
Keyboard/caret EOF reveals add a single line-height bottom inset before
clamping, so the final display row is not painted exactly on the viewport
clip edge.

### No Paint-Time Worker Wait

Paint polls `try_use_worker_result_rich` exactly once per frame. A stamp-matched worker result is accepted immediately; every miss goes straight to the inline realization for the already-classified build kind. Large cold/dirty/splice paths use viewport-priority partial row-index walks, so the fallback is microseconds-to-low-ms and the current contract does not include a paint-time worker wait.

The projection worker still publishes into its latest-result cell and still wakes the condvar inside `ProjectionWorker::wait_for_result_publication`, but production paint does not wait on that condvar. `event:worker_wait`, `event:worker_resubmit`, and `event:paint_loading_overlay` no longer emit in current traces.

## Hit-testing (mouse)
`Window::on_mouse_lbuttondown(x, y)` is the other consumer of `FrameDisplay`. Every mouse → buffer-position mapping routes through `Window::resolve_hit_test_frame_display` (`crates/ui/src/window_mouse_hit_test.rs`) so `client_to_buffer_position`, `segment_hit_at_client`, and `cursor_over_ctrl_click_target` share **one** resolved projection per click:
1. Resolve which pane the (x,y) is inside (via `compute_leaf_rects`); a non-focused pane is focused first by `try_pane_body_focus_switch`.
2. `resolve_hit_test_frame_display` picks the projection in this order:
   - reuse `last_painted_frame_display` when `PrewarmQuery::is_compatible_for_hit_test` matches (looser than motion-compat — rope and decoration revision drift is ignored because the click maps to what the user *saw*).
   - else `SpectatorFrameCache::lookup_for_hit_test_with_reason` keyed by `tree.focused` — strict hit-test compat (same document / fold / wrap / font). On miss, emits `event:spectator_cache_lookup path=hit_test result=miss miss_reason=…` (`no_entry` / `document` / `fold_signature` / `wrap_width_dip` / `font_state`).
   - else `SpectatorFrameCache::lookup_same_document` (wrap-tolerant) — bypasses the wrap field because, after a `try_pane_body_focus_switch`, the focused-pane wrap formula (`resolve_body_text_width_dip`) differs from the spectator's (`pane_body::spectator_body_text_width_dip`), so the strict arm misses on the very click that triggered the focus switch. The cached frame's wrap matches what's currently painted (paint hasn't run at the new wrap yet), so the click maps to the pixels the user pointed at; resolution is in source bytes (wrap-independent) so downstream consumers see no drift. Trace token `click_hit_test_frame_source source=spectator_cache_wrap_tolerant`.
   - else reuse the focused-pane mouse hit-test fallback cache (`source=mouse_cache`) when the query is still hit-test compatible. This is the frame seeded by an earlier hover / click fallback before paint had a chance to run.
   - else consult the row-index cache and materialize only viewport + overscan specs when a compatible index is present. If the row-index cache also misses, the fallback builds a source-line-floor frame with a synthetic one-row-per-source-line index so the input handler never runs the whole-document row-count walker.
3. Resolve which display line via `y / line_height + view.scroll_y`.
4. Use the cached layout's `HitTestPoint(x_local, y_local)` → utf16 hit position.
5. Convert to source bytes via `DisplayLineSpec::display_to_source`.
6. Build a `Selection`, dispatch `SetSelections` to core.

Trace label `click_hit_test_frame_source` records the arm (`last_painted` / `spectator_cache` / `spectator_cache_wrap_tolerant` / `mouse_cache` / `viewport_build` / `cache_empty`). The resolver is shared by `client_to_buffer_position` and `segment_hit_at_client`, so one click cannot cold-build separate full-document `FrameDisplay`s for each lookup. Sharing the resolver, tolerating revision drift, and adding the wrap-tolerant fallback for the focus-switch click together kill the in-click doubling. The fallback cache also carries the frame and decoration context forward to the next paint: when `last_painted_frame_display` and prewarm both miss, `window_paint::mouse_candidate` can promote it as `CachedFrameSource::MouseHitTest` so paint dirty-rebuilds from that frame instead of repeating the same cold row-index walk. Trace token: `mouse_hit_test_frame_display hit=rebuild_candidate shadowed=true|false` or `miss=field_…`.
Segment hits first query the shared P18 `SegmentCache` with the same line-projection stamp the row-count walker uses. On cache miss they reuse the same resolver and derive the row directly from `client_y + scroll_y` before reading `FrameDisplay::display_line_by_index`, so wrapped continuation rows do not map through the first source-line row. A click or hover must not emit `event:row_count_walker reason=mouse_hit_test`; any nearby walker row belongs to paint or prewarm.

## Where each step lives

| Step | File |
|---|---|
| Wndproc dispatch | `crates/ui/src/window.rs::wndproc` |
| Paint orchestration | `crates/ui/src/window_paint.rs`; tail validation in `crates/ui/src/window_paint/epilogue.rs` |
| FrameDisplay build | `crates/render/src/display_projection.rs::FrameDisplay::build` |
| Document-end scroll | `Window::pending_doc_end_scroll` set in `crates/ui/src/window_commanding/context.rs`, applied in `crates/ui/src/window_paint/doc_end_scroll.rs` after `resolve_paint_frame_display`, with post-`EndPaint` repaint scheduling in `crates/ui/src/window_paint/epilogue.rs` |
| Scroll bounds / scrollbar | `crates/ui/src/window_runtime.rs`, `crates/ui/src/window_view.rs`, `crates/ui/src/window_scroll.rs`, `crates/render/src/scrollbar.rs` |
| DisplayMap builder | `crates/display_map/src/builder.rs` (+ `builder/segments.rs`) |
| Renderer | `crates/render/src/renderer.rs::Renderer::draw_buffer` |
| Layout cache | `crates/layout/src/cache.rs::LayoutCache` |
| Caret rect | `crates/render/src/chrome_caret.rs::caret_rect_for_shape` |
| Current-line band | `crates/render/src/chrome.rs::paint_current_line_highlight` |
| Wrap-mode body | `crates/render/src/wrap_paint.rs::paint_display_lines` |
| Spell squiggles | `crates/render/src/spell.rs::paint_spell_spans` |
| Pane chrome | `crates/render/src/pane_chrome.rs::paint_pane_chrome` |
| Non-focused panes | `crates/render/src/pane_body.rs::paint_all_pane_bodies` |
| Status bar | `crates/render/src/status_bar.rs::paint_status_bar` |
| File tree | `crates/render/src/file_tree_paint.rs::paint_file_tree` |
| Overlay panel | `crates/render/src/overlay.rs::paint_overlay`, `crates/render/src/overlay_scrollbar.rs`, `crates/render/src/overlay_motion.rs::paint_overlay_with_motion` |
| Motion inputs | `crates/render/src/motion.rs`, `crates/render/src/jump_glow_paint.rs` |

## Debug recipe: a source line "won't render correctly"

When a row appears missing, blank, mis-styled, or stale and the obvious code path looks right, several **independent** transforms compose into the final pixels. Check all of them — not just the one that named the feature:

| Mechanism | Effect on the source line | Where it composes |
|---|---|---|
| `display_map::table_hide_provider` and sibling per-line helpers (`backslash_escape_provider`, …) | mark byte ranges `Action::Hide` or `Action::Replace`; the line **stays** 1 display row | `crates/display_map/src/builder/segments.rs` |
| `Decorations::inlines` `MarkerKind::*` spans | `Hidden`/`Replaced` per the caret-inside-block reveal rule | `crates/display_map/src/builder/segments.rs` |
| `&[FoldRange]` (the projection's fold input) | source line **collapses to zero display rows** when fully covered | composed by `ui::Window::display_projection_folds` (`crates/ui/src/window_display_prewarm.rs`) |
| `&[ImageRowReservation]` | injects phantom display rows *after* the natural row | `crates/display_map/src/image_row_reservation_provider.rs` + builder dispatch |
| Caret-inside-block rule (per feature) | reveals marker bytes / suppresses visual-chrome painters | feature-specific predicates |
| Painter chrome (`table_paint`, `inline_color_paint`, …) | overlays D2D paint *over* the rendered glyphs. The focused-pane table chrome is retained: `table_chrome_cache` records one `ID2D1CommandList` per visible table before `BeginDraw`, `renderer_table_chrome::run_replay` `DrawImage`s each after the body glyph pass. Spectator panes still use the per-line `table_paint` painter | `crates/render/src/*_paint.rs`, `crates/render/src/table_chrome_cache.rs`, `crates/render/src/renderer_table_chrome.rs` |
| Block-decoration painters (`paint_block_backgrounds`, `paint_horizontal_rules`) | thin overlays for fenced-code panels, blockquote bars, and horizontal rules. Y math goes through `FrameDisplay`: `first_display_row_of_source_line × line_height` rather than `source_line × line_height`. Soft-wrap continuation rows above an HR / panel push the painted divider down by the same total; a fully folded source line returns `None` from the `visible_display_row` helper and the painter skips it. Multi-line block panels cover the inclusive display-row range of their source lines. Spectator pane bodies do not invoke these painters today — raw `---` glyphs render unmodified | `crates/render/src/decoration_paint.rs` |
| Layout-cache key (`content_stamp` + revision) | stale `IDWriteTextLayout` from before a content change | `crates/render/src/text_helpers.rs::build_key_for_spec` |
| Decoration freshness | undecorated frames produce raw markdown until the worker catches up; results are pumped by both `on_paint` and the 250 ms watchdog tick | `crates/ui/src/window_decoration.rs`, `window_decoration_watchdog.rs` |
| Decoration parse-revision drift (`decoration_parse_advanced`) | a fresh worker parse arriving at the same rope rev as a stale-transformed prior decoration would otherwise hit the covering-cache fast path and silently drop the new styling — the flag tracks the worker's actual parse rev separately from the transformed `Decorations::revision` label and forces a rebuild when they diverge | `crates/ui/src/window_paint.rs::on_paint` (samples + sets `Window::last_painted_decoration_parse_revision`); classifier guard in `crates/ui/src/window_projection_plan.rs::classify_projection_build` |
| Large dirty-set spill (`LARGE_DIRTY_SET_THRESHOLD`) | a `ProjectionBuildKind::Dirty` whose dirty set exceeds 1500 lines (e.g. after a fresh tree-sitter full-parse of a 100 k-line buffer) is paint as `prev` (or a viewport-only cold build when prev doesn't realise the viewport) and submitted to the projection worker; the worker chews through the full rebuild off-thread and `try_use_worker_result` accepts the rebuilt frame on the next paint. Trace event: `paint:frame_display:dirty_spilled dirty_count=… threshold=… covers_viewport=…` | `crates/ui/src/window_projection_plan.rs::realize_projection_build_kind` |
| Cold-deferred focused stub | when `classify_projection_build` picks `Cold` AND the worker has no stamp-matched result AND a same-rope/same-decoration cached frame exists at a *different* `wrap_width_dip` AND the buffer is ≥ `COLD_DEFERRED_STUB_LINE_THRESHOLD` (2000) source lines, paint substitutes the cached frame for one paint instead of cold-walking the row index inline; the post-paint dispatch already submits Cold to the worker so the next paint accepts the real new-wrap frame. The stub is NOT seeded into `last_painted_frame_display` or the spectator cache (its row positions are at the old wrap; a motion-compat lookup keyed by the new wrap would otherwise return geometrically-invalid layout data). Trace events: `paint:frame_display:cold_deferred stub_wrap=… target_wrap=… rope_rev=… reason=…` on substitution; `paint:frame_display:cold_deferred_skip reason=no_candidate\|buffer_too_small\|wrap_width_match\|rope_revision_drift\|decoration_revision_drift` when the helper refuses. | `crates/ui/src/window_paint/cold_deferred.rs`, `crates/ui/src/window_paint/frame_resolution.rs` |
| Mouse hit-test paint candidate | when a click into a large pane had to build a hit-test fallback frame before paint, the frame is stored with its `PrewarmQuery`, decorations, and decoration parse revision. The fallback reuses a compatible row-index cache entry when available; on row-index miss it builds a source-line-floor frame instead of running the walker. The next paint may promote the frame after `last_painted_frame_display` and prewarm miss, then `classify_projection_build` treats it like `LastPaint` for selection-reveal and dirty rebuild decisions. Trace token: `mouse_hit_test_frame_display`. | `crates/ui/src/window_mouse_hit_test.rs`, `crates/ui/src/window_mouse_hit_test_cache.rs`, `crates/ui/src/window_paint/mouse_candidate.rs`, `crates/ui/src/window_paint/frame_resolution.rs` |
| Caret display-row lookup | `Window::resolve_caret_display_line` reuses `last_painted_frame_display` only when motion-compatible, otherwise uses a cached `DisplayRowIndex` through the viewport builder. If the exact row-index cache misses after a stable-shape edit, it refreshes only the new caret line plus previous caret-reveal lines from the prior painted row index, then materializes the caret line's display-row range. A viewport-realized frame can have a correct whole-document row index while the caret source line's specs are not realized; that is `resolution=row_index_only`, not a fold. If no same-shape previous index exists, the resolver returns a source-line floor estimate, so caret capture/restore does not invoke the walker inline. Trace: `caret_display_line_lookup source=… resolution=realized_spec\|row_index_only\|folded_fallback display_row=… total_rows=… realized=A..B`. | `crates/ui/src/window_caret_anchor.rs`, `crates/ui/src/window_caret_row_index.rs` |
| Cold / partial-build sub-spans | the full inline cold path (row-index cache miss on a small buffer or explicit full build) splits `paint:frame_display:cold_build` into `event:row_count_walker dur_us=… reason=paint_cold\|paint_dirty\|viewport_realize\|prewarm` and `event:viewport_materialize dur_us=…`. Large buffers route through `ColdPartial`, `DirtyPartial`, or `SplicePartial` instead and emit `event:partial_row_index_walk`, `event:partial_dirty_walk`, or `event:partial_splice_walk` with requested/walked source-line counts and estimated total rows. `event:row_count_walker_stats` reports per-decision-path counters for both full and partial walker calls. | `crates/ui/src/window_display_prewarm/frame_build.rs::cold_build_with_split_trace`, `crates/ui/src/window_display_prewarm/frame_build_partial.rs`, `crates/display_map/src/builder/row_counts.rs::WalkerStats`, `crates/render/src/text_metrics.rs::DirectWriteWidthMeasure` |
| Spectator no-wrap stub | when a no-wrap spectator buffer is ≥ `SPECTATOR_STUB_LINE_THRESHOLD` (2000) source lines, `build_spectator_pane_data` keeps the cache/build under `wrap_width_dip = 0`; that is geometrically current because soft wrap is disabled and the walker emits one row per source line without DirectWrite wrap measurement. Trace token: `paint:spectator_cache miss=stub_no_wrap`. | `crates/ui/src/window_paint_spectators.rs` |
| Spectator current-geometry partial | when `SpectatorCacheLookup::Empty` or `Stale(_)` resolves and `Window::has_pending_spectator_projection` reports an exact-stamp worker request in flight, spectator paint builds a current-geometry partial frame for the visible source range instead of `FrameDisplay::placeholder_unrealized(...)` or a full inline cold walk. Large stale misses use the same partial path outside the pending-worker arm, including `wrap_width_dip`, `decoration_revision`, `rope_revision`, and `image_reservations_signature` drift; reservation-bearing 2x2 panes therefore do not run a full spectator row-count walk on the UI thread when image or table row reservations change. Stale-geometry partials are paint-only and are not inserted into `SpectatorFrameCache`; same-geometry `realized_miss_partial` frames are inserted because they only extend a compatible cached frame's realized row window. Spectator worker requests compute the same per-pane image / table-row reservations and table suppression as paint, so the worker-drained full frame refills the cache under the reservation-bearing query that the next paint uses. Spectator paint uses the body rect as viewport truth and clamps saved scroll only when the visible range is entirely past the frame's display-row count, so near-EOF focus changes do not move a still-visible pane. Trace: `paint:spectator_cache miss=realized_miss_partial`, `miss=placeholder_pending_partial`, `miss=wrap_width_partial`, `miss=live_resize_partial`, or `miss=stale_partial`. The worker drain (`Window::drain_spectator_projection_worker_results`) runs after paint geometry is prepared and refills the cache on result acceptance, scheduling a repaint. | `crates/ui/src/window_paint_spectators.rs`, `crates/ui/src/window_paint_spectators/realtime_miss.rs`, `crates/ui/src/window_projection_spectator.rs` |
| Caret visibility fast path | `Window::ensure_primary_caret_visible` is a post-edit guard, not a reflow anchor. It first estimates the caret row from geometry-compatible `last_painted_frame_display`, the mouse hit-test frame cache, or the spectator cache using hit-test compatibility (document / fold / wrap / font; rope/decor rev drift tolerated). Same-line edits therefore avoid a foreground row-count walk while paint/worker catches up. The exact resolver still handles `with_caret_line_anchored` capture/restore and centered navigation. Trace token: `ensure_primary_caret_visible_fast source=last_painted_hit_test|mouse_hit_test_cache|spectator_cache|source_line_floor`. | `crates/ui/src/window_view.rs`, `crates/ui/src/window_caret_anchor.rs` |
| Document-end motion | Ctrl+End / Shift+Ctrl+End moves the selection and then sets `Window::pending_doc_end_scroll`. The generic post-command `ensure_primary_caret_visible` lands the viewport approximately near the new caret so the next paint's cold build covers the bottom rows. Then `on_paint`, after `resolve_paint_frame_display` produces the canonical `frame_display`, snaps `scroll_y_dip` to `frame_display.display_line_count() * LINE_HEIGHT_DIP + END_OF_BUFFER_BOTTOM_PADDING_DIP - viewport_h`. The one-line inset keeps the last row inside the viewport instead of exactly on the clip edge. If the snap moves the view, the already-resolved paint draws with the pre-snap scroll and schedules `reason=doc_end_snap` after `EndPaint`, so the corrected-bottom repaint cannot be validated away and a top-realized frame is never rendered at a bottom scroll offset. Using the painter's own projection eliminates the row-count divergence (image reservations, fold ranges, decoration revision drift) that previously made a command-thread cold rebuild land short. | `crates/ui/src/window_paint.rs`, `crates/ui/src/window_commanding.rs`, `crates/ui/src/window_commanding/context.rs` |

**Fold composition is the high-leverage trap.** `display_projection_folds` is the single authoritative composition site for *every* fold input that reaches the display map. Adding a new "hide this kind of row" helper as a fold range there will silently collapse the row to **zero** display rows — making it disappear entirely rather than rendering as an empty slot. If you intend to keep the slot but blank the glyphs, write a per-line provider next to `table_hide_provider` (slot stays, bytes become `Hidden`). Reserve fold ranges for genuine user-toggled / structural collapses.

**Past incident.** A `window_table_alignment_fold.rs` helper composed fold ranges for every pipe-table `|---|---|` delimiter row. Bytes were "hidden" but the row also fully collapsed, so body rows slid up against the header and downstream display-map / painter edits had no row to paint into. The fix was to delete the fold helper and rely on `table_hide_provider` (keep the slot) plus a chrome painter (`table_paint::paint_alignment_row_dividers`) for the styled strip.
