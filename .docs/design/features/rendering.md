# Rendering

DirectWrite layout cache + Direct2D paint pipeline + DXGI swap chain. Body text, gutter, pane chrome, tab strip, status bar, overlays, and inline images all paint from a single `FrameDisplay` projection built off the rope + display map + decoration snapshot. Keypress → pixel budget: 8 ms p99.

## What it is
- Direct2D + DirectWrite compositor over a DXGI swap chain (one per window). PerMonitorV2 DPI awareness. Layout cache keyed by `(buffer, line, revision, font_state, soft_wrap_width)`, where `font_state` includes the per-window DPI scale — typically ~10× visible lines per pane, LRU evicted.

## Key concepts
- **Swap chain per window** — one `IDXGISwapChain` per window, frame-latency waitable for vsync. Main top-level windows opt out of DWM redirection bitmaps with `WS_EX_NOREDIRECTIONBITMAP`; the renderer owns the full client visual through DXGI/D2D, and the opt-out avoids stale redirection-surface strips during live shrink.
- **DIP coordinate space — renderer is the sole DIP→pixel converter** — the whole app (layout, fonts, margins, `body_origin`, viewport, wrap width, mouse hit-test) works in 96-DPI device-independent pixels; `scaled_font_size` applies user zoom only, never DPI, and the DirectWrite text format takes a DIP size. The renderer is the single place that turns DIPs into physical pixels, and it does so through the D2D device context: **every back-buffer target bind calls `ID2D1DeviceContext::SetDpi(window_dpi, window_dpi)`**, so all draw primitives scale by `window_dpi / 96`. The target bitmap's matching `dpiX/dpiY` only fixes the DIP size the bitmap *reports* (`GetSize`); it does **not** drive the drawing transform, so the explicit `SetDpi` is required. Without it the context paints at the default 96 DPI (1 DIP = 1 px) and the DIP-native layout fills only `client_px / scale` of the back buffer — the dark right/bottom band seen at 125 %+. `dpi_scale` is carried in `DrawParams` / `FontStateId` / the chrome- and table-chrome-cache keys **only** to invalidate caches on a monitor move, never as a manual geometry multiplier — there is no `* dpi_scale` in the draw geometry.
- **`IDWriteTextLayout` cache** — bounded LRU; entries reused when revision matches.
- **`FrameDisplay`** — per-frame `Arc<DisplayMap>` projection (see `display-map.md`). The renderer paints from display bytes, not source bytes.
- **Per-line `SetTransform`** — Phase 17.6 cleanup tail. Body decorations + layout paint at layout-local `(0, 0)`; chrome resets to identity. Both the source-line path and the soft-wrap display-line path run the F3 inline-color and F4 table-formula overlay passes.
- **Retained static chrome** — ruler columns, the status-bar shell, and the outline-sidebar shell are recorded into an `ID2D1CommandList` and replayed with `DrawImage` on warm paints. The key is `(theme_revision, dpi_scale, pane_geometry_hash, sidebar_visibility, minimap_visible, outline_visible)`. Body text, selections, decoration overlays, caret, inline images, minimap content, pane labels, and motion overlays remain per-frame.
- **Retained table chrome (P14.1)** — markdown pipe-table cell fills, header / alignment-row backgrounds, 1-DIP cell borders, and per-column-aligned cell text are recorded *per visible table* into their own `ID2D1CommandList`s, replayed between the body glyph pass and the spell / focus-dim / chrome-post passes (so squiggles and indent guides still overdraw cells correctly). The key is `(document, block_start, layout_content_hash, theme_revision, font_state, dpi_scale, line_height, base_font_size)`. Bounded LRU at 64 entries; invalidated on resize alongside the static-chrome list. Only the focused pane runs through the cache today — spectator panes keep the per-line painter because they may carry independent fonts / widths.
- **Spectator pane** — every visible non-focused pane body paints inside its own clip rect via `pane_body::paint_all_pane_bodies`, including the same left gutter reservation as the focused body. Spectators paint from their own per-pane `FrameDisplay`; soft-wrap walks display rows exactly like the focused pane instead of relying on DirectWrite source-line reflow. Per-pane projections are cached UI-side by `ui::window_spectator_cache::SpectatorFrameCache` keyed by `PrewarmQuery`, so a non-focused pane holding a large rope reuses its last painted projection while the focused pane receives keystrokes — only true projection-input drift (rope / decoration revision, wrap width, font, folds) triggers a rebuild. When a compatible cache entry misses only because its realized row window does not cover the current viewport, paint builds a bounded `realized_miss_partial` and seeds that extension back into the spectator cache so focus-return / scroll repeats hit on the next paint. See `display-map.md` § Operations → "Spectator-pane reuse". A never-seen-wrap miss on a ≥ `SPECTATOR_STUB_LINE_THRESHOLD` (2000) source-line buffer substitutes a `wrap_width_dip = 0` no-wrap stub frame for one paint (the walker becomes O(N) without DirectWrite); the stub bypasses both caches so a later focus-promote cannot reuse the wrong geometry. Trace token: `paint:spectator_cache miss=stub_no_wrap`.
- **Cold-deferred focused stub** — when `classify_projection_build` picks `Cold`, the first non-blocking worker poll misses, the buffer is ≥ `COLD_DEFERRED_STUB_LINE_THRESHOLD` (2000) source lines, and a same-rope/same-decoration cached frame exists at a *different* `wrap_width_dip` (a focus switch + new pane-width combo, typically), paint substitutes the cached frame for one paint instead of cold-walking the row index. The post-paint dispatch already submits Cold to the worker; the next paint accepts the real new-wrap frame and reseeds the caches. The stub is NOT seeded into `last_painted_frame_display` or the spectator cache (its row positions are at the old wrap; a motion-compat lookup would otherwise return geometrically-invalid layout data). Small buffers cold-build inline because the walker is fast enough that the stub's brief wrap shift is pure visual cost. Trace events: `paint:frame_display:cold_deferred` on substitution; `paint:frame_display:cold_deferred_skip reason=no_candidate|buffer_too_small|wrap_width_match|rope_revision_drift|decoration_revision_drift` when the helper refuses. See `paint-flow.md` § "No Paint-Time Worker Wait".
- **Mouse hit-test frame promotion** — when `resolve_hit_test_frame_display` has to build a fallback frame before paint, the UI stores it with its `PrewarmQuery` plus decoration context. Segment-click handling checks the shared P18 `SegmentCache` before resolving a fallback frame. The fallback uses a compatible cached row index when present and a source-line-floor frame when the row-index cache misses, so input handling does not run the whole-document walker. The next paint can promote it as `CachedFrameSource::MouseHitTest` after last-paint and prewarm miss, then dirty-rebuild from it instead of repeating projection work. Trace token: `mouse_hit_test_frame_display`.
- **Motion draw inputs** — render is stateless for motion. UI projects `SurfaceMotion`, `StatusTransientDraw`, and `JumpGlowDraw` into `DrawParams`; render only paints the supplied frame.
- **File-tree draw input** — UI projects `FileTreeState` into `FileTreeDraw` containing only visible rows. Render paints it as left chrome in the post-body pass below modal overlays.
- **Smooth wheel scroll (P12)** — vertical wheel input feeds a UI-thread-owned inertia accumulator (`crates/ui/src/window_scroll.rs`). Plain wheel input resolves the screen-coordinate `WM_MOUSEWHEEL` point into this window's client space, targets the hovered pane body (including gutter / line-number / minimap / scrollbar chrome inside that pane), and leaves keyboard focus unchanged. Non-pane surfaces, active overlays, and active drag mechanisms claim the wheel before pane routing. `[editor].mouse_wheel_scroll_speed` multiplies the base 3-line notch distance before either inertia or reduced-motion instant scrolling; default `2.0` yields 6 lines/notch. The existing scroll animation timer (`SCROLL_ANIM_TIMER_ID`, ~60 Hz) advances the target pane's `ViewState.scroll_y_dip` fractionally with a `60 ms` time constant; velocity below 50 DIP/s snaps to zero. Paint translates the body by the fractional `scroll_y_dip` and renders glyphs at `display_row * line_height - scroll_y_dip`. Inertia is cancelled by every owner that takes over scroll state (edits, clicks, scrollbar drag, buffer/pane switch, DPI change, anchored reflow). Wheel and scrollbar bounds clamp against `FrameDisplay::display_line_count() * line_height`; keyboard/caret EOF reveals add one line-height bottom inset so the final row is visible instead of sitting exactly on the clip edge. Reduced motion uses the same configured speed for a whole-line instant jump and never starts decay.
- **Strip realize during scroll-anim (P12)** — when a scroll-anim paint reuses a compatible cached frame but the live viewport's uncovered row gap fits within `SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS = 80`, paint extends the previous frame's realized window by materializing only the leading-edge strip rows through `Window::rebuild_frame_display_dirty` (the same call site `ViewportRealize` uses). Realized strip rows seed the cache. When the gap exceeds the budget, paint falls back to a low-contrast placeholder strip over the unrealized rows (theme key `editor.loading_overlay.background` attenuated to 60 % alpha, neutral grey fallback) and lets the sliding-window prewarm fill in. Decision (pure helper in `window_paint/frame_resolution/scroll_anim_action.rs`): `realized.covers(visible) ? Reuse : uncovered_rows <= 80 ? StripRealize : Placeholder`.
- **Sliding-window scroll prewarm (P12)** — once per paint, while inertia is active, if the realized window holds less than `0.5 × viewport_height` ahead of live scroll in the direction of velocity, paint submits one worker request for the next-page-ahead viewport via the early-dispatch path (`submit_reason=scroll_prewarm`). The worker channel's latest-wins dedupe drops back-to-back duplicates; velocity reversal flips the leading edge the predicate measures. Trace tokens: `event:projection_worker_early_dispatch reason=scroll_prewarm submit_reason=scroll_prewarm` and `event:projection_worker_queue_depth reason=scroll_prewarm`.
- **Partial-frame fill prewarm** — cache-hit paints may reuse a partial `FrameDisplay` whose visible rows are correct but whose row index is not fully realized. Paint still submits a background `ProjectionPlan::Cold` with reason `paint_partial_fill` for that same stamp so the next paint can reuse a full index. Focus changes apply the same rule through `window_projection_early_dispatch/focus_prewarm.rs`: if focus lands on a partial cache hit or a partial classify result, early dispatch submits Cold immediately. The worker's same-pane coalescer retains an older `paint_partial_fill` request beside a newer same-revision non-fill request, but drops stale fills when rope / font / wrap / document drift makes them obsolete.
- **Caret visibility after edits** — `Window::ensure_primary_caret_visible`
  keeps typing from drifting off-screen without forcing the exact
  caret-anchor projection on every keystroke. It reuses geometry-compatible
  painted / mouse-hit-test / spectator `FrameDisplay` row indexes for a
  projection-backed row estimate, then falls back to the exact resolver only
  when no cache can safely answer. Reflow anchors and centered navigation
  still use the exact `resolve_caret_display_line` path. Ctrl+End /
  Shift+Ctrl+End defer the exact bottom snap to the paint path:
  `editor.move_doc_end` / `editor.extend_doc_end` set
  `Window::pending_doc_end_scroll` after moving the caret, then `on_paint`
  snaps `scroll_y_dip` against the canonical paint-frame `FrameDisplay`
  and re-invalidates so the follow-up paint cold-builds the corrected
  bottom viewport.

## Per-frame pipeline

```
1. UI builds FrameDisplay from latest snapshot + decoration cache.
2. renderer.draw_buffer(rope, selections, &mut cache, &DrawParams)
   ├─ BeginDraw + Clear(bg)
   ├─ Per-line LayoutCache::get_or_build keyed by content_stamp
   ├─ PushAxisAlignedClip body content `[0, 0, viewport_w - margins.right, viewport_h]`
   │   (locks in world coords; persists across body sub-transforms)
   ├─ Current-line band (B16: spans all wrapped rows for the caret's source line)
   ├─ Trailing-whitespace fill
   ├─ Per-line paint loop:
   │   - SetTransform to layout-local
   │   - Draw selection rects under text
   │   - Apply theme-driven footnote drawing effects to footnote runs
   │   - DrawTextLayout
   │   - Draw caret(s) (caret_rect_for_shape — B4 width-aware)
   │   - Draw link / checkbox decorations
   │   - Spell-check squiggles (delegated to crate::spell)
   ├─ Retained per-table chrome replay (P14.1): one DrawImage per visible focused-pane table; the command lists were recorded before BeginDraw
   ├─ Phase-11 chrome: indent guides, gutter
   ├─ PopAxisAlignedClip (body-content right edge)
   ├─ Scaled-text minimap (right column, inside outline when both on)
   ├─ Search-active minimap strip (rides minimap column when both on)
   ├─ Phase-16.5 non-focused pane bodies (each in its own clip rect)
   ├─ α.0 jump-glow destination-row tint, if active
   ├─ Phase-13 chrome: tab strips, pane borders (focused accent + active-tab motion)
   ├─ Retained static chrome command list (ruler columns + static shells)
   ├─ Phase-18 status bar text/chips (background shell retained)
   ├─ Left file-tree pane, if `DrawParams::file_tree` is set
   ├─ Overlay panel (palette, find bar, …) with motion if present
   ├─ Chord HUD overlay with motion if present
   └─ EndDraw + Present(1, 0)
```

Wrap-mode paint goes through `crate::wrap_paint` for the focused pane and `crate::pane_body`'s display-row loop for spectators; both share the same display-projection input and markdown-extension overlays.

## Data model

```rs
struct DrawParams<'a> {
    document:    Option<&'a DocumentChrome>,
    format:      &'a IDWriteTextFormat,
    font_state:  FontStateId,
    theme_revision: u64,
    dpi_scale: f32,
    line_height: f32,
    base_font_size_dip: f32,
    heading_scale: [f32; 6],
    view:        &'a ViewState,
    colors:      EditorColors,
    markdown_colors: MarkdownColors,
    view_options: ViewOptionsDraw<'a>,
    decorations: Option<&'a Decorations>,
    loading_overlay: Option<&'a LoadingOverlayDraw>,
    loading_overlay_motion: Option<SurfaceMotion>,
    overlay:     Option<&'a OverlayDraw>,
    overlay_motion: Option<SurfaceMotion>,
    chord_hud:   Option<&'a OverlayDraw>,
    chord_hud_motion: Option<SurfaceMotion>,
    jump_glow:   Option<JumpGlowDraw>,
    body_origin: (f32, f32),
    pane_chrome: Option<&'a PaneChromeDraw>,
    spell_spans: &'a [SpellSquiggleSpan],
    pane_bodies: &'a [PaneBodyDraw<'a>],
    frame_display: &'a FrameDisplay,
    client_height_dip: f32,
    file_tree: Option<&'a FileTreeDraw>,
}

enum CaretShape { Bar, Block, Underline }
struct ViewOptionsDraw {
    line_numbers, gutter_caret_line_only, current_line_highlight,
    indent_guides, whitespace_markers, trailing_whitespace,
    minimap, indent_size, ruler_columns,
    caret_shape, caret_visible, caret_bar_width_px,
    show_status_bar,
}
```

## Operations
- **Layout cache**: `LayoutCache::get_or_build(key, &dwrite_factory, layout_text, format, font_size, line_height, soft_wrap_width, syntax_highlights, markdown_colors, heading_scale)` returns a cached `IDWriteTextLayout`. Cache key includes `FontStateId` so font reloads invalidate consistently.
- **Deferred font swap** (no-overflow font commits): family / size commits route through `Window::request_font_change(family, size_dip)` rather than mutating `prose_font_family` / `font_size_dip_override` directly. The helper stores a `PendingFontChange { target_family, target_size_dip, target_font_state }` on `Window::pending_font_change` and returns. While the pending change is in flight, every projection-stamp call site uses `Window::effective_font_state()` (which returns `pending.target_font_state` when present, else `self.font_state`), so the projection worker builds the display map for the new font in the background. Paint keeps rendering the previous font against the previous `frame_display` — no overflow flash. At the top of each `on_paint`, `Window::try_apply_pending_font_swap(focused_pane)` peeks the worker's mailbox via `ProjectionWorker::peek_latest_result_font_state_for_target` (non-consuming); when the queued result's `font_state` matches `pending.target_font_state`, the swap fires inside `with_caret_line_anchored`: mutates `prose_font_family` / `font_size_dip_override`, calls `invalidate_font_state()` to drop `text_format`, clears pending, and arms `Window::font_swap_settle_deadline = now + 2 s`. `ensure_renderer` then rebuilds `text_format` for the new family within the same paint, and the regular `take_latest_result_for_target` drain hits because the post-swap stamp now matches the queued result. Commit sites: `confirm_font_picker` (Enter), the `ChooseFontW` fallback, `set_font_size_impl`, and both font branches in `apply_settings`. Picker arrow-step preview stays on the instant-swap path (`set_font_family`) since per-row deferral would cost 17–480 ms cache lag per highlight move.
- **Spectator settle nudge** (post-swap catch-up for non-focused panes): `try_apply_pending_font_swap` only peeks the focused pane's mailbox. After the swap, non-focused panes' worker results for the new `font_state` may still be in flight; the worker thread does not post a wake to the UI thread on result publication, so without intervention the spectator caches stay on the old font_state until the next mouse / focus / scroll event. `Window::nudge_font_swap_settle` runs at the end of every `on_paint`: while `font_swap_settle_deadline` is unexpired and `SpectatorFrameCache::any_entry_lags_font_state(self.font_state)` returns true, it calls `invalidate_hwnd`. The loop terminates as soon as every spectator entry catches up *or* the 2-s watchdog elapses (whichever comes first). The existing `drain_spectator_projection_worker_results → populated → invalidate_with_reason("spectator_cache_populate")` chain takes over once any spectator result actually lands. Both gates (`Some(deadline)` and `any_entry_lags`) must be true for the nudge to fire, so steady state is zero-cost.
- **Markdown span styles**: `text_helpers::ensure_line_layout_for_spec` bakes `SpanStyle` font weight/style/scale into each `IDWriteTextLayout`. Footnote references use `SpanStyle::footnote` (smaller font scale) and `apply_footnote_drawing_effects` applies the theme's `markdown.footnote` brush at draw time so theme reloads do not require rebuilding cached layouts.
- **Caret rect**: `chrome_caret::caret_rect_for_shape(caret_x, line_y, line_height, column_advance, shape, bar_width_px)`. `bar_width_px == 0` keeps the legacy 1.5 DIP.
- **Current-line band**: `chrome::paint_current_line_highlight(..., display_rows: Option<(u32, u32)>)`. Wrap-aware via `(first_display_row, row_count)`.
- **Status bar**: `status_bar::paint_status_bar(...)` consumes `StatusBarData`; caller passes `client_height_dip - STATUS_BAR_HEIGHT_DIP` as `top`. `compute_layout` snaps every segment / chip `left` and `right` bound to the nearest integer DIP — `slot_width_dip` is `chars * font_size * 0.55` and would otherwise accumulate fractional offsets that make ClearType re-sample changing-digit text against a different sub-pixel grid each paint (perceived as blurry counters). With rounding, same-text repaints are pixel-identical.
- **Status transients**: `StatusBarData::transients` carries localized chip fade/slide overlays for changed values. Renderer paints only the changed segment/chip, not the entire bar. The UI gates transient scheduling on `is_motion_eligible_kind` — high-frequency counters (`Position`/`Chars`/`Words`/`Lines`/`Selection`/`NumericSum`) are suppressed so the 180 ms slide-and-fade doesn't double-image during typing. See `motion.md` § Status Chips.
- **File tree**: `file_tree_paint::paint_file_tree` consumes `FileTreeDraw`. Labels are one-line `IDWriteTextLayout`s with wrapping disabled and per-label clipping, so long paths cannot overpaint later rows.
- **Retained static chrome**: `chrome_command_list::ChromeCommandList` creates an `ID2D1CommandList` with `ID2D1DeviceContext::CreateCommandList`, records static D2D primitives when its invalidation key changes, and replays with `ID2D1DeviceContext::DrawImage`. `Renderer::resize_for_hwnd` invalidates the list before rebinding the swap-chain target; device-loss reconstruction creates a fresh renderer and therefore a fresh cache.
- **Retained table chrome**: `table_chrome_cache::TableChromeCache` owns one `ID2D1CommandList` per visible markdown pipe-table. `renderer_table_chrome::prepare_retained_chrome` runs both this cache *and* the static-chrome list before the renderer's outer `BeginDraw` so neither inner `BeginDraw`/`EndDraw` pair nests; `renderer_table_chrome::run_replay` walks each visible table after the body-glyph pass, installs a per-table screen transform, and `DrawImage`s the cached list. Cache key carries `(document, block_start, layout_content_hash, theme_revision, font_state, dpi_scale, line_height, base_font_size)`. `Renderer::resize_for_hwnd` invalidates the cache. Stats surface as `event:table_chrome_path` and the `chrome_overlay_table_us` field of `event:renderer_draw_stages`. Immediately after replay, `renderer_table_chrome::paint_focused_active_cell_outlines` walks `params.table_layouts` × selections and paints a 2-DIP outline + intra-cell caret bar (or translucent fill when the selection fully covers a cell) on top of the cached chrome — caret-dependent state that can't sit in the cache. See `features/tables.md`.
- **Spell squiggles**: `crate::spell::paint_spell_spans(...)` consumes `spell_spans: &[SpellSquiggleSpan]` and translates source ranges into display utf16 coordinates via `FrameDisplay::source_byte_in_line_to_display_utf16`.
- **Pane chrome**: `crate::pane_chrome::paint_pane_chrome` paints tab strips, pane borders, focused-pane accent, and supplied active-tab/focus motion frames. Tab labels are DirectWrite layouts with no wrapping, clipped to the tab slot before the close glyph paints.
- **Loading overlay painter**: `crate::loading_overlay::paint_loading_overlay` consumes a `LoadingOverlayDraw` payload when supplied. Current production paint does not arm the UI-side `LoadingOverlayState`, so `DrawParams::loading_overlay` is normally `None`; the renderer helper remains available for explicit transient-banner payloads.
- **Overlay draw**: `crate::overlay::paint_overlay` paints focused-field selection ranges before input glyphs, then the caret only when `OverlayDraw::input_focused` is true. List rows paint next; `OverlayDraw::scrollbar` paints after rows for capped palette-style lists. `paint_overlay_with_motion` applies the supplied opacity/translation frame first.
- **Renderer resize reuse**: `Renderer::resize_for_hwnd(hwnd, w, h)` rebinds only the swap-chain back buffer + D2D target bitmap at the HWND's live DPI and re-issues `ID2D1DeviceContext::SetDpi(dpi, dpi)` so the DIP→pixel drawing transform tracks the new DPI (the bitmap DPI alone does not — see § Key concepts); the D3D11 device, DXGI factory, D2D factory/device/context, DirectWrite factory, and image cache are reused across WM_SIZE and WM_DPICHANGED. The legacy `Renderer::resize(w, h)` and the WARP capture path bind at 96 DPI, so the §D pixel canary stays at 1:1. The HWND swap chain uses `DXGI_SCALING_STRETCH` so DWM can stretch the last presented frame during the brief gap between a client-size change and the next paint. `Window::refresh_client_size` falls back to the cold `Renderer::for_hwnd` construction path only when target rebind returns an error (e.g. device removed). On live shrink-axis ticks that still fit inside the current target, the target rebind is deferred until `Window::paint` has built the replacement frame and is about to call the renderer; grow ticks still resize immediately. Per-tick layout-cache purging on resize is **not** performed — the focused-pane wrap-paint key is `wrap_width_dip = 0`, so spectator pane width churn is bounded by the LRU rather than by eager invalidation. See `cache.rs::invalidate_other_wrap_widths` doc comment for the trap that motivates this.
- **Live drag-resize fast path**: between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`, `refresh_client_size` takes a cheap viewport-update path (`refresh_focused_viewport_unanchored`) and skips the per-tick caret anchor. A single anchor captured at `WM_ENTERSIZEMOVE` is restored once at `WM_EXITSIZEMOVE` against the final projection. Both shrink and grow motions force a full-client `InvalidateRect(hwnd, None, false)` per delta because the window class is registered without `CS_HREDRAW`/`CS_VREDRAW` and `DefWindowProc` only invalidates newly-exposed regions on grow. Grow ticks resize the swap-chain target immediately; shrink ticks that still fit inside the current target defer target rebind until paint is ready to draw the replacement frame. The main HWND uses `WS_EX_NOREDIRECTIONBITMAP`, the window procedure returns handled for `WM_ERASEBKGND`, live `WM_SIZE` calls `UpdateWindow` after invalidating so the repaint is delivered before control returns to the modal sizing loop, and shrink-axis ticks call `DwmFlush` after that repaint so the compositor consumes it before the sizing loop advances.

## Configuration
- `editor.font_family_prose` (B-defaults: Segoe UI Variable) / `font_family_mono` (Cascadia Mono → Consolas).
- `editor.font_size`, `editor.line_height`.
- `editor.caret_*` (shape, blink, width — see `caret.md`).
- `editor.ruler_columns`, `editor.ligatures`, `editor.smooth_scroll`, `editor.mouse_wheel_scroll_speed`, `editor.zoom_step_pct`.
- `ui.show_status_bar`, `ui.show_line_numbers`, `ui.show_minimap`.
- `ui.reduced_motion` disables all motion inputs at the UI layer; render receives final static state only.
- All hot-reloadable via the settings watcher.

## Key files
- swap chain + draw loop: `crates/render/src/renderer.rs`
- static chrome command list: `crates/render/src/chrome_command_list.rs`
- per-table chrome cache: `crates/render/src/table_chrome_cache.rs`
- per-table chrome orchestration (prep + replay around `BeginDraw`): `crates/render/src/renderer_table_chrome.rs`
- renderer construction (cold path): `crates/render/src/renderer/construction.rs`
- renderer resize reuse (hot path): `crates/render/src/renderer/resize.rs`
- chrome (gutter, current-line, indent guides, trailing-whitespace, ruler, minimap): `crates/render/src/chrome.rs`, `chrome_post.rs`, `chrome_caret.rs`
- per-line wrap-aware paint: `crates/render/src/wrap_paint.rs`
- decoration paint helpers: `crates/render/src/decoration_paint.rs`
- pane bodies: `crates/render/src/pane_body.rs`
- pane chrome: `crates/render/src/pane_chrome.rs`
- scrollbars and scroll bounds: `crates/render/src/scrollbar.rs`, `crates/ui/src/window_scroll.rs`, `crates/ui/src/window_view.rs`
- spell squiggles: `crates/render/src/spell.rs`
- loading overlay painter: `crates/render/src/loading_overlay.rs`
- overlay (palette, find, goto): `crates/render/src/overlay.rs`, `crates/render/src/overlay_scrollbar.rs`
- motion draw structs/helpers: `crates/render/src/motion.rs`, `crates/render/src/overlay_motion.rs`, `crates/render/src/jump_glow_paint.rs`
- text helpers (utf8↔utf16 + layout style runs): `crates/render/src/text_helpers.rs`
- text metrics: `crates/render/src/text_metrics.rs`
- params: `crates/render/src/params.rs`
- status bar: `crates/render/src/status_bar.rs`
- file tree payload + painter: `crates/render/src/file_tree.rs`, `crates/render/src/file_tree_paint.rs`
- display projection: `crates/render/src/display_projection.rs`
- layout cache: `crates/layout/src/cache.rs`
- projection worker partial-fill coalescing: `crates/ui/src/projection_worker/worker_loop.rs`
- focused-paint partial-fill dispatch: `crates/ui/src/window_paint/dispatch.rs`
- focus-change partial-fill prewarm policy: `crates/ui/src/window_projection_early_dispatch/focus_prewarm.rs`
- deferred font swap (request + try-apply + spectator settle nudge): `crates/ui/src/window_font_swap.rs`
- non-consuming worker peek for the swap check: `ProjectionWorker::peek_latest_result_font_state_for_target` in `crates/ui/src/projection_worker.rs`
- spectator-lag detection accessor: `SpectatorFrameCache::any_entry_lags_font_state` in `crates/ui/src/window_spectator_cache.rs`; `PrewarmQuery::font_state` accessor in `crates/ui/src/display_prewarm_cache.rs`

## Relates to
- [Display map](display-map.md) — every `IDWriteTextLayout` is built from `DisplayLineSpec`.
- [Theme](theme.md) — colors flow in via `EditorColors` + `MarkdownColors` snapshots.
- [Decoration](decoration.md) — block + inline + heading spans bake into layout attributes.
- [Caret presentation](caret.md) — `CaretShape`, blink, width are renderer inputs.
- [Motion](../motion.md) — render consumes projected motion frames but does not own timers or animation state.
- [Performance](../performance.md) — keystroke → pixel budget and the caches that defend it.
- [File tree](file-tree.md) — left folder browser payload and safety caps.
- [Settings](settings.md) — font-family / font-size commit paths persist via the writeback helpers *and* defer the visible swap until the worker delivers; the two flows compose at the commit site.
