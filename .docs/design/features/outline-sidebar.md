# Outline sidebar + markdown TOC (§F2)

Right-docked per-pane sidebar listing the active buffer's heading tree, plus two markdown commands that generate / refresh an in-document table of contents bounded by HTML-comment markers.

## Surface

- View toggle: `view.toggle_outline` (no default chord) flips the current buffer's runtime outline visibility. `[ui].show_outline_sidebar` supplies the default for buffers without an override. Width comes from `[ui].outline_sidebar_width_dip` (default 220 DIP). Settings hot-reload.
- Drag-resize: dragging the sidebar's left edge (a ±4 DIP grab band, `IDC_SIZEWE` cursor) resizes it live, clamped to `120..=min(600, 80 % of body)` DIP; the settled width persists to `[ui].outline_sidebar_width_dip` on release and prewarms the projection at the new wrap width (the sidebar consumes body width, so resizing reflows text). Drag state is `MouseState.outline_resize_drag`; logic in `window_outline_resize.rs`. Routed before the row hit-test so a grab on the band never doubles as a heading jump.
- TOC commands: `markdown.insert_toc` writes a fresh marker-delimited bullet list at the caret line. `markdown.refresh_toc` re-runs the formatter in place against the existing `<!-- toc --> … <!-- /toc -->` pair; no-op when the markers are absent.
- Theme keys: `editor.outline.{background, foreground, foreground_active, separator}`. Required on every bundled theme + the neutral fallback.
- Click target: a click anywhere inside a sidebar row scrolls the heading line to the viewport top and places the caret at column 0 of that line. Hit-test consults a layout the paint pass caches in `view_options.outline_layout`.

## Pipeline

```
decorate::headings::headings(spans, source) ─┐
                                             ├─▶ ui::window_paint::build_outline_entries
decorate::sections::heading_index_at         ─┤      → Vec<OutlineEntry>
                                             │
ViewOptions.show_outline_sidebar  ───────────┘
                                             ▼
                            OutlineData { entries, current_index, colors,
                                           width_dip, font_size_dip }
                                             ▼
       render::outline_paint::paint_outline_frame (D2D fill + per-row text)
                                             ▼
                  render::compute_outline_layout → cached in
                          view_options.outline_layout for click hit-test
```

`headings(spans, source)` is the existing tree-sitter-md walk — no extra parse per paint. `heading_index_at` provides the caret → row mapping for the follow-caret highlight (`OutlineColors.fg_active`).

## Body-rect subtraction

When `show_outline_sidebar` is true and soft-wrap is on, `window_paint` shrinks the projection wrap width by `outline_sidebar_width_dip` so glyphs do not flow under the strip. The strip itself paints over the right edge of the focused pane's body rect.

## TOC formatting

`decorate::toc::format_toc(headings)` emits:

```
<!-- toc -->
- [Foo](#foo)
  - [Bar](#bar)
<!-- /toc -->
```

Slugs follow GFM rules (`slugify` in `decorate::toc`): lowercase, strip non-alphanumeric except `-`/`_`, runs of whitespace → single `-`, trim leading/trailing `-`, disambiguate duplicates with `-1`/`-2`/…. Indent is two spaces per level past the shallowest heading in the filtered list.

`find_toc_block(source) -> Option<(start_byte, end_byte)>` locates an existing block by the marker pair so `markdown.refresh_toc` can drop the body and rebuild without disturbing surrounding text. Both mutation commands route through `EditorHandle::apply_edit`; each call lands as one undo group.

## Layout, clipping, scrolling

Outline rows use a fixed `OUTLINE_ROW_HEIGHT_DIP` stride. The renderer
pushes a D2D axis-aligned clip around the sidebar rect; partial top /
bottom rows are clipped rather than bleeding into the editor body.
Long headings set `DWRITE_WORD_WRAPPING_NO_WRAP` and clip horizontally
at the row's right edge — they do not wrap onto a second visual line.
The list gets a 4-DIP scrollbar indicator when content height exceeds
sidebar height. Mouse-wheel over the sidebar updates
`view_options.outline_scroll_offset_dip` and consumes the wheel so the
editor body does not scroll alongside. The cached layout
(`OutlineLayout`) now carries `content_height_dip` and
`scroll_offset_dip` and is consulted by both paint and click hit-test.

## Buffer-local right-edge visibility

Outline and minimap visibility are runtime right-edge chrome state
keyed by `BufferId`, not by pane or window. `Window` owns
`right_edge_chrome_defaults` plus
`right_edge_chrome_by_view: HashMap<BufferId, RightEdgeChromeState>`.
Settings reload updates the defaults from `[ui].show_outline_sidebar`
and `[ui].show_minimap`; buffers with explicit runtime overrides keep
their override.

`view.toggle_outline` mutates the focused scalar
`ViewOptions.show_outline_sidebar`, records the resulting
`(minimap, outline)` pair for `Window::buffer_id`, clears right-edge
geometry caches, and submits layout-change projection prewarm. Focus
switch, tab adoption, reopen, layout shortcut, and time-machine buffer
swap reload the focused scalar mirror from the active buffer's
right-edge state before paint or hit-test reuse. Switching buffers
within a pane clears stale outline / minimap / search-minimap geometry
caches so a new buffer does not inherit old click rectangles.

## Spectator outline + minimap

Non-focused pane bodies reserve right-edge width and paint outline /
minimap from the active buffer's own right-edge state. Spectator
outline entries are rope-backed (no `rope.to_string()` clone) and
built from the visible pane's decoration snapshot; decorations not
ready paints an empty strip rather than blocking.

Left-click outline navigation and minimap click / drag target the
focused pane only today. Right-click over a visible outline or minimap
in any pane opens a one-item chrome context menu. The UI focuses the
pointed-at pane before dispatching `view.toggle_outline` or
`view.toggle_minimap`, so the toggle updates that pane's active buffer.
Cursor over outline / minimap chrome is the default arrow, not the text
I-beam.

## Outline entries cache

`Window::outline_entries_cache` (`window_outline_entries_cache.rs`) is
the UI-thread cache keyed by `(BufferId, rope_revision,
decoration_revision)`. Outline paint and outline click hit-test share
the same cache; entry construction extracts heading slices from the
rope per-heading via `rope.byte_to_line` rather than materializing a
`String` of the whole buffer. Empty-heading snapshots are still
cached with `decoration_revision = None` so a later decoration
delivery misses and rebuilds. Trace event:
`event:outline_entries cache=hit|miss rope_rev=N
decoration_rev=M|none entries=K elapsed_us=…`.

## Display-map classifier extension

Outline / minimap toggles route their reflow through
`view_toggle_outline` / `view_toggle_minimap` early dispatch
(`event:projection_worker_early_dispatch reason=toggle_outline|toggle_minimap`).
The wrap-width partial-eligible classifier locks in
`large_wrap_width_change_routes_to_cold_partial` so a sidebar toggle on
a large buffer takes the partial-realize path instead of cold-walking
the full row index. See `display-map.md` for the partial classifier.

## Deferred / out-of-scope for §F2

- **Keyboard focus into the sidebar**: spec §F2 mentions it but the chord binding belongs with §H4 (focus mode); the sidebar paint pass already reserves a slot.
- **Collapsed thin-chevron mode**: not painted yet. When `show_outline_sidebar` is false, no strip and no chevron — toggle via the command. A future revision can paint a 10-DIP chevron rail using the `separator` color so the strip is discoverable.

## Files

- `crates/decorate/src/toc.rs` — TOC formatter + `find_toc_block`.
- `crates/decorate/src/headings.rs` — heading extractor.
- `crates/decorate/src/sections.rs` — `heading_index_at` for the follow-caret row.
- `crates/render/src/outline.rs` — pure layout + hit-test.
- `crates/render/src/outline_paint.rs` — D2D paint dispatch.
- `crates/ui/src/window_paint.rs` — per-frame `OutlineData` build + body-width subtraction.
- `crates/ui/src/window_outline.rs` — click hit-test, scroll handling, TOC mutation handlers.
- `crates/ui/src/window_outline_resize.rs` — left-edge drag-resize (grab band, live width, width persistence).
- `crates/ui/src/window_outline_entries_cache.rs` — UI-thread `(BufferId, rope_revision, decoration_revision)` cache shared by paint + click.
- `crates/ui/src/window_right_edge_chrome.rs` — buffer-local minimap / outline state + chrome hit-test.
- `crates/ui/src/window_context_menu.rs` — right-click chrome menu dispatch.
- `crates/ui/src/window_paint_builders.rs` — spectator pane minimap / outline state collection.
- `crates/ui/src/window_paint/payload.rs` — `PaneBodyDraw` assembly for spectators.
- `crates/ui/src/window_view_toggles.rs::toggle_outline_impl` — view toggle.
- `crates/ui/src/window_view_options.rs` — `show_outline_sidebar`, `outline_sidebar_width_dip`, cached `outline_layout`.
- `crates/render/src/params.rs` — spectator right-edge flags in `PaneBodyDraw`.
- `crates/render/src/pane_body.rs` — spectator margin, wrap-width, outline, and minimap paint.
- `crates/command/src/view.rs` — `view.toggle_outline` registration.
- `crates/command/src/markdown.rs` — `markdown.insert_toc` / `markdown.refresh_toc` registration.
- `crates/config/src/settings.rs` — `[ui].show_outline_sidebar` + width defaults.
- `crates/theme/src/keys.rs` + `crates/theme/src/theme.rs` — `editor.outline.*` keys + accessors.
