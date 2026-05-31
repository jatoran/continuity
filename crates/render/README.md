# render

Direct2D compositor over a DXGI swap chain (one per window). Draws
selections / cursors / decorations behind and in front of `IDWriteTextLayout`
output. No custom glyph atlas — DirectWrite manages glyph caches.

Layer: middle. Depends on `layout` and `win`.

Pure layout + paint modules live as `foo.rs` / `foo_paint.rs` siblings.
F1 ships `breadcrumb.rs`; F2 ships `outline.rs` + `outline_paint.rs`;
the left file-tree pane ships `file_tree.rs` + `file_tree_paint.rs`;
the UI supplies only visible rows and the painter clips each one-line
label to its row so long paths cannot overlap; the scaled-text minimap
ships `minimap.rs` + `minimap_paint.rs`
(`paint_minimap_scaled` called from `Renderer::draw_buffer_no_present`
when `view_options.minimap` is set — one tiny `IDWriteTextLayout` per
visible source line at `MINIMAP_FONT_SIZE_DIP`, plus a viewport-indicator
overlay. See `.docs/design/features/minimap.md`).
(right-docked heading sidebar — `paint_outline_frame` is called from
`Renderer::draw_buffer_no_present` when `view_options.show_outline_sidebar`
and `params.outline` are both set). `table_layout.rs` + `table_paint.rs`
ship the pipe-table visual-cell renderer: when the caret is outside a
table block, `compute_table_layouts` produces a `Vec<TableLayout>` and
the painter draws 1-DIP cell borders, header backgrounds, column-aligned
text, and evaluated formula values on top of the display map's
pipe-hidden projection. The byte-level F4 swap painter
(`table_formula_paint.rs`) skips any block that has a visual layout to
avoid double-drawing. See `.docs/design/features/tables.md`.

Markdown footnotes use the display-map's `SpanStyle::footnote` runs. The
layout builder bakes the smaller font scale into cached `IDWriteTextLayout`s;
the per-frame text pass applies the theme's `markdown.footnote` brush before
`DrawTextLayout` so color follows live theme reloads.

Soft-wrap projection uses `DirectWriteWidthMeasure` on the UI thread whenever
the window has an active text format. The display-map still owns the pure row
split, but the widths now come from the same font family, size, weight, and
heading scale that `DrawTextLayout` paints; `FixedCharWidth` is only the
fallback for tests and pre-render setup.

Motion inputs are immutable paint data. `motion.rs` defines
`SurfaceMotion`, `StatusTransientDraw`, `JumpGlowDraw`, and `EditPulseDraw`;
`loading_overlay.rs` defines `LoadingOverlayDraw`, the render-side payload
for an explicitly supplied transient loading banner.
`overlay_scrollbar.rs` paints the optional list scrollbar carried by
`OverlayDraw` for capped palette-style overlays. `overlay_motion.rs`,
`jump_glow_paint.rs`, `edit_pulse_paint.rs`, `loading_overlay.rs`,
`pane_chrome.rs`, and `status_bar.rs` consume those
values for the current frame. Timers, reduced-motion policy, stagger
scheduling, and the
range/duration math for the edit pulse remain in `ui`
(`crates/ui/src/edit_pulse.rs`).
`pane_chrome.rs` keeps tab labels one-line and clipped to their tab
slot so long derived titles cannot wrap into the strip or cover the
close glyph.

`image_cache.rs` is the WIC-decoded D2D bitmap cache for inline images.
It stores one bitmap per source frame (`frames: Vec<ID2D1Bitmap1>`):
static images have `frames.len() == 1`; animated GIFs decode every
frame upfront with parallel `frame_delays_ms`. `advance_animations(now_ms)`
walks every animated entry, multi-stepping `frame_index` to converge on
the right frame if the caller's timer fell behind. `has_animated_entries()`
lets the UI auto-disarm the timer when the cache drops to all-static.
The `Renderer` surface (`advance_image_animations`, `has_animated_images`)
hides the `RefCell` from callers. Per-frame GIF delay metadata
(`/grctlext/Delay`) is a known follow-up — see
`.docs/design/features/tutorial.md` Status table; v1 plays every frame
at `DEFAULT_FRAME_DELAY_MS = 100ms`.

`render_stats.rs` is a trace-only aggregation helper. The UI builds a
`RenderStats` snapshot around `Renderer::draw_buffer*` when
`CONTINUITY_UI_TRACE` is enabled, producing one `paint:render_stats`
line with layout-cache deltas, focused/spectator row counts, and
feature toggles/counts. It also carries `ChromePathStats` (emitted as
`event:chrome_path mode=fresh|replay elapsed_us=N`) and
`TableChromePathStats` (emitted as
`event:table_chrome_path mode=fresh|replay tables=N fresh=N replay=N
record_us=N replay_us=N elapsed_us=N`); the same per-table elapsed
time is surfaced as `chrome_overlay_table_us` inside
`event:renderer_draw_stages`. The renderer still does not own logging
or trace sinks.

`chrome_command_list.rs` retains static chrome in an
`ID2D1CommandList`. `Renderer::draw_buffer_no_present` computes the
invalidation key at paint entry, records the list only when the theme,
DPI scale, pane geometry hash, sidebar visibility, minimap visibility, or
outline visibility changes, and replays it with `DrawImage` during the
post-body chrome stage. Currently retained primitives are ruler columns,
the status-bar shell background, and the outline-sidebar shell fill and
separator. Dynamic chrome text, pane labels, selection/caret/body paint,
minimap content, inline images, and motion overlays stay in the
per-frame path.

`renderer/resize.rs` rebinds the swap-chain back buffer and D2D target
without rebuilding D3D/D2D/DirectWrite. The renderer tracks target width,
height, and DPI for cheap no-op detection; the HWND swap chain uses
`DXGI_SCALING_STRETCH` so the compositor can cover the gap between a
resize tick and the next presented paint. The UI may defer live shrink
target rebinds until the paint path is ready to draw, keeping the old
presented frame available during projection work.

`renderer_draw_main.rs` carries the body of `draw_buffer_no_present`
extracted out of `renderer.rs` to hold the 600-line cap. It brackets
eleven chrome-overlay sub-stages with `Instant` scopes and stashes a
`RendererChromeOverlayBreakdown` on the renderer cell at end of draw;
the breakdown surfaces on `event:renderer_draw_stages` as eleven
`chrome_overlay_*_us` fields plus `chrome_overlay_sum_us`. The chrome
overlay accounting contract (sum within 5 % of `chrome_overlay_us`)
holds by construction.

`scrollbar.rs` computes focused-pane scrollbar geometry from
`FrameDisplay::display_line_count()`. UI passes the matching raw content
height into `ViewState` clamps, so scroll max is derived from actual
display rows instead of an estimated EOF slack range.

`display_projection/placeholder.rs` carries
`FrameDisplay::placeholder_unrealized(source_line_count, revision,
decoration_revision, wrap_width_dip, font_state) -> Self` — a paint-only
`FrameDisplay` with an empty realized window. Used by the spectator
path when an exact-stamp worker request is already pending: paint
substitutes the placeholder for one paint instead of cold-walking,
and the renderer fills unrealized rows with the existing
scroll-placeholder strip via `PaneBodyBrushes.placeholder`. The
placeholder is **not** inserted into `SpectatorFrameCache`; only the
real worker result refills the cache.

`scroll_placeholder.rs` + `renderer_scroll_placeholder.rs` paint a
low-contrast strip over visible rows the cached `FrameDisplay` has not
realized during inertial scroll (P12 cold-flick fallback). Strip color
attenuates the theme's `editor.loading_overlay.background` to 60 %
alpha or falls back to a neutral semi-transparent grey. Pure helpers
(`compute_unrealized_strips`, `paint_scroll_placeholder_strips`) are
unit-tested; the D2D wrapper sits in the renderer module to keep brush
construction local.

`tab_drag_paint.rs` paints the four tab-drag drop affordances when
`PaneChromeDraw.tab_drag` is supplied: source-strip insertion bar
(2 DIP accent), source-tab fade overlay (60 % strip background),
pane-body drop highlight (6 % accent tint + 2 DIP border, inset
4 DIP), and the cursor-attached tear-off ghost (panel-bg fill +
accent border + label, ~80 % opacity). Both the focused-window and
the broadcast-driven foreign-side overlays go through the same
painter.

`code_copy_button_paint.rs` paints the fenced + inline `` `code` ``
hover affordance (`CodeCopyButtonDraw`). Idle/hovered glyph is `⎘`,
copied `✓`, failed `✕`; "Copied" tint fills with the active theme's
caret accent at α=0.92; foreground glyph picks near-black/near-white
via a BT.601 luma threshold for legibility across themes. The button
paints after the scrollbar / gutter and before overlays.
`inline_code_paint.rs` paints the inline `` `code` `` background fill
(soft-wrap path publishes per-frame client-DIP hit rects via
`Renderer::inline_code_hits()` for hover detection; the no-wrap
painter renders the background only).

`decoration_paint.rs` translates block-decoration Y math through
`FrameDisplay`: `paint_block_backgrounds` and `paint_horizontal_rules`
take `frame_display: &FrameDisplay` and resolve source-line →
display-row via `visible_display_row` / `block_display_span` helpers.
Multi-line panels cover the inclusive display-row range; folded
source lines paint nothing. The longest-line measurement clips the
fenced-block highlight at content width + 12 DIP padding so short
snippets no longer paint a viewport-wide band.

`table_chrome_cache.rs` extends the same record/replay idea to
markdown pipe-table cell chrome — cell fills, header / alignment-row
backgrounds, 1-DIP cell borders, per-column-aligned cell text. Each
visible table records its own `ID2D1CommandList` keyed on
`(document, block_start, layout_content_hash, theme_revision,
font_state, dpi_scale, line_height, base_font_size)`. The cache is
bounded by a 64-entry LRU (oldest `last_used_frame` evicted on
insert), invalidated on resize, and lives on the focused-pane path
(`Renderer::draw_buffer_no_present`'s body loop). The orchestration
(visibility cull, record before `BeginDraw`, replay after the body
glyph pass and before spell / focus-dim / chrome-post) lives in
`renderer_table_chrome.rs`. Spectator panes still paint table chrome
per source line via `pane_body.rs` + `table_paint.rs`.
