# Minimap

Right-docked, scaled-down rendering of a pane's active buffer. Drives nothing on its own — it is a thumbnail navigation aid that mirrors the body text at roughly 1/12 horizontal scale, with a translucent box marking the section currently on screen. `[ui].show_minimap` supplies the default for buffers without an override; `view.toggle_minimap` flips the current buffer's runtime right-edge chrome state.

## What it is
- A vertical column on the right edge of every editor pane whose active buffer has minimap visibility enabled. Paints one tiny `IDWriteTextLayout` per source line at `MINIMAP_FONT_SIZE_DIP` (~2.4 DIP) so the reader can navigate by silhouette.
- A translucent **viewport indicator** rectangle painted over the strip's section that corresponds to the visible body region. Tracks scroll continuously. Its size + travel are driven off the editor's **display-row** content height (`content_height_dip`), scrollbar-consistent — not the source-line count, which would drift from the mouse whenever soft-wrap is on (the default).
- Width: `MINIMAP_WIDTH_DIP` (80 DIP). Reserved on the right margin of the body by `ContentMargins::from_view_options` so glyphs in the editor never overlap the strip.
- Docks **inside** the outline sidebar when both are active (outline outermost; minimap immediately to its left).

## Layer split
- Pure layout math + constants in `crates/render/src/minimap.rs` (`MINIMAP_WIDTH_DIP`, `MINIMAP_FONT_SIZE_DIP`, `MINIMAP_LINE_HEIGHT_DIP`, `MINIMAP_INNER_PADDING_DIP`, `compute_minimap_layout`, `hit_test`). Unit-testable without a swap chain.
- D2D + DirectWrite paint in `crates/render/src/minimap_paint.rs::paint_minimap_scaled`. Re-creates a minimap-sized `IDWriteTextFormat` per frame off the base format's font family; bounded by visible-line count, so cost scales with pane height (not buffer size).

## Layout math
`compute_minimap_layout(pane_rect, scroll_y_dip, line_height_dip, total_lines, content_height_dip, right_inset_dip)` returns:

- `rect` — strip outer rect in pane-local DIPs. `x = pane_rect.x + pane_rect.w - right_inset_dip - MINIMAP_WIDTH_DIP`.
- `indicator_rect` — viewport-indicator overlay box. Proportional thumb/track: height is the visible fraction of `content_height_dip`, travel by scroll `progress` — identical to the scrollbar's notion of progress.
- `(scroll_y_dip, font_size_dip, line_height_dip)` — what the painter feeds DirectWrite.
- `first_visible_line` / `last_visible_line` — half-open range the painter iterates so off-strip lines never build a layout.

`content_height_dip` is the editor's **display-row** content height (`display_row_count * line_height`); the indicator + scroll geometry track it so the strip stays consistent with the editor scroll (which lives in display rows) under soft-wrap / folds / reserved rows. It falls back to `total_lines * line_height` when a caller passes `0.0` (no projection yet). The per-line glyph paint still walks **source** lines (the minimap renders source content); the discrepancy is absorbed by the proportional indicator + click math, mirroring the scrollbar.

When `total_lines * MINIMAP_LINE_HEIGHT_DIP > pane_h`, the minimap glyph column scrolls proportionally to the same display-row `progress` so it stays aligned with the visible region. When the full minimap fits, `scroll_y_dip = 0` and every line paints in place.

## Why a separate text format
DirectWrite's `IDWriteTextFormat` locks its font size at creation — we can't `SetFontSize` to scale the body's cached layouts down. Two viable options:
1. Re-use the body's per-line cached layout and apply a `SetTransform` scale matrix → glyphs render at the right pixel size but layout metrics are still computed at body size, leaving wide whitespace in the minimap column.
2. Build a minimap-sized `IDWriteTextFormat` and a fresh `IDWriteTextLayout` per visible line → glyphs *and* metrics scale together; the strip reads as a thumbnail.

We took (2). The minimap layout cache could be added later if `dhat` flags the per-frame allocations — they're bounded by visible-line count, not buffer size, so the cost is constant in steady state.

## Click + drag

Left-click in the minimap strip scrolls the editor so the clicked
**display-row** point is centered in the viewport; left-drag continues
applying the same mapping until button-up or capture loss. The click is
resolved **proportionally** in display-row space, scrollbar-consistent:
the fraction of the strip track the click sits at maps to the same
fraction of the editor's scrollable range. Resolving against source
lines (the pre-§28 behavior) drifted from the mouse whenever soft-wrap
was on. `minimap_hit_test(layout, x, y, content_height_dip,
viewport_height_dip)` returns a `MinimapHit` whose `target_scroll_dip`
is already clamped to `[0, content - viewport]`; the click handler
(`window_minimap.rs::scroll_to_minimap_point`) applies it directly via
`view.jump_to` — there is no separate target-scroll helper (the former
`compute_minimap_target_scroll` was removed). `MinimapHit.line` is a
best-effort source-line hint for traces only. Hit-testing reads
`view_options.minimap_layout`, a per-paint cache of the last
`MinimapLayout` so click / drag does not recompute layout math.
Left-click and drag target the focused pane only today.

Right-click over any visible minimap opens a one-item chrome context
menu: `Toggle Minimap`. If the minimap belongs to a non-focused pane,
the UI focuses that pane before dispatching `view.toggle_minimap`, so
the toggle updates the pointed-at buffer. Cursor over a minimap is the
default arrow, not the text I-beam.

Trace event: `event:minimap_click target_dip=<f32>
target_buffer_y=<f32> scrolled=true|false`.

## Buffer-local visibility
- Runtime owner: `ui::Window` owns `right_edge_chrome_defaults` plus `right_edge_chrome_by_view: HashMap<BufferId, RightEdgeChromeState>`.
- Defaults: hot-reloaded from `[ui].show_minimap` and `[ui].show_outline_sidebar`; buffers without an override inherit the defaults.
- Toggle: `view.toggle_minimap` mutates `Window::view_options.minimap`, then records the resulting `(minimap, outline)` pair for `Window::buffer_id`.
- Focus switch / tab adoption: the focused scalar mirror reloads minimap visibility from the active buffer before layout caches are reused.
- Split panes: non-focused pane collection snapshots the active buffer's minimap flag into `PaneBodyDraw`; spectator wrap width, margins, and paint consume that field instead of the focused pane's `view_options`.

## Coexistence rules
- **Body width is shrunk.** `ContentMargins::from_view_options` accumulates `right += MINIMAP_WIDTH_DIP` (or `SEARCH_MINIMAP_WIDTH_DIP` when only the search strip is active) + `outline_sidebar_width_dip`. Text reflows so glyphs never sit under the strip.
- **No-wrap text is clipped.** `renderer.rs::draw_buffer_no_present` pushes an axis-aligned clip `[0, 0, viewport_w - margins.right, viewport_h]` around the body-paint loop, popped just before the minimap paints. Long lines without soft-wrap get cut at the strip's left edge instead of bleeding into it.
- **Outline outermost.** When `show_outline_sidebar` is on, the minimap insets by `outline_sidebar_width_dip` so the two columns don't overlap. Same convention applies to the search-active minimap strip in `crates/ui/src/search_minimap.rs::build_layout`.

## Theme keys
- `editor.minimap.background` — strip fill.
- `editor.minimap.foreground` — scaled-glyph color. Usually a partial-alpha foreground tint.
- `editor.minimap.viewport_indicator` — translucent box drawn over the visible section.

All three are required and declared in `crates/theme/src/keys.rs`. Bundled themes (`paper`, `solarized_*`, `deep_minimal`) each ship values.

## Performance
- Per-frame work: O(visible_lines_in_strip × short_layout_build). With `MINIMAP_LINE_HEIGHT_DIP = 2.7` and a 600-DIP pane, that's ~222 small `IDWriteTextLayout`s per frame in the worst case — well inside the §15 keypress→pixel budget. Each layout is dropped at the end of the frame.
- Per-line character cap of 240 UTF-16 units so pathological 100k-column lines can't pin a CPU on layout construction.
- Search-active minimap ticks are separate from the scaled-text minimap. `crates/ui/src/search_minimap.rs` buckets dense match lists to at most one tick per vertical DIP while preserving the active match tick, so a huge find result cannot turn chrome paint into O(matches).

## Tests
- `crates/render/src/minimap.rs::tests` — `short_buffer_does_not_stretch_minimap_lines` (the regression-fix invariant), `long_buffer_scrolls_minimap_proportionally`, `indicator_uses_display_row_content_height` (§28 — taller display content ⇒ shorter indicator), `indicator_rect_tracks_visible_portion`, `strip_docks_at_right_edge_minus_inset`, `hit_test_resolves_proportional_scroll_target` / `hit_test_top_click_scrolls_to_origin` / `hit_test_outside_strip_returns_none`.
- `crates/ui/src/search_minimap.rs::tests::outline_inset_shifts_strip_inward` — companion check for the search-strip dock when both sidebars are active.
- Pixel canary (`crates/render/tests/pixel_canary.rs`) covers the visible output for full integration.

## See also
- [Rendering](rendering.md) — paint pipeline, clip lifecycle.
- [Outline sidebar](outline-sidebar.md) — the outer right-edge consumer.
- [Search](search.md) — the search-active minimap strip that rides this column.

## Key files
- `crates/ui/src/window_right_edge_chrome.rs` — buffer-local right-edge state + chrome hit-test.
- `crates/ui/src/window_view_toggles.rs::toggle_minimap_impl` — command implementation + override recording.
- `crates/ui/src/window_context_menu.rs` — right-click chrome menu dispatch.
- `crates/ui/src/window_paint_builders.rs` — non-focused pane state collection.
- `crates/ui/src/window_paint/payload.rs` — `PaneBodyDraw` assembly.
- `crates/render/src/params.rs` — spectator minimap flag in `PaneBodyDraw`.
- `crates/render/src/pane_body.rs` — spectator margin, wrap-width, and minimap paint.
