# Tables — visual rendering + cell interaction

Pipe-table blocks render as visual cells **unconditionally** — caret position no longer toggles between raw markdown and visual chrome. The user never sees raw `| a | b |` pipes; cells are always laid out with borders, header background, per-column alignment, and formula-evaluated values. Interaction is spreadsheet-style: click a cell to select its content, double-click to position the caret for editing, right-click for row/column structural ops.

The alignment row keeps its source-line slot. Its bytes are hidden and `table_paint` draws a divider strip (body_bg + per-column borders) so the row reads as part of the bordered region. Source-line ↔ display-line stays 1:1 for single-row tables; a wrapped or `<br>` row reserves >1 display row (see [Multi-line cells](#multi-line-cells--wrapping-phase-f)), and the display map keeps the gutter / caret below it aligned.

## Pipeline

1. **`continuity_decorate::table_eval`** — already produces `Decorations::evaluated_tables: Vec<EvaluatedTable>` per revision. Each `EvaluatedTable` carries the block's byte range and per-cell formula overrides (`=SUM(...)` → computed value). Formula evaluation uses a memoizing `ChainEvaluator` (`table_eval/chain.rs`) that lazily resolves cell dependencies on demand and caches numeric results within one pass. `=B1+3` in a cell where `B1 = =SUM(A1:A3)` sees the resolved sum rather than the literal-only matrix's `None`. Cycles surface as the `#CIRC` sentinel — once a cell is flagged cyclic, downstream references read it as empty (`0`), so an unrelated formula that mentions the cyclic cell still computes a stable value. Sentinels: `#CIRC` (cycle), `#DIV/0!` (divide-by-zero), `#ERR` (other formula errors). Self-reference (`=A1` in A1) and mutual reference are detected without recursion.

2. **`continuity_display_map::table_hide_provider`** — sibling module called by `builder/segments.rs`. Emits `Hidden` ranges for every `|` byte on non-delimiter rows, every formula override's source payload, and the entire delimiter row, for every table block. **No caret gating** — hides are unconditional. Escaped `\|` is preserved. Hiding formula payloads at the display-map layer keeps raw formula tails out of normal text layout and soft-wrap row counts; `table_layout` still consumes the canonical rope + `EvaluatedTable` overrides and paints the computed value.

3. **`continuity_render::table_layout`** — pure function `compute_table_layouts(tables, rope, caret_bytes, suppressed_table_blocks, measure) -> Vec<TableLayout>`. Walks every table block source line-by-line, classifies header / alignment / body rows, runs `column_alignments` for `:---: / ---: / :---` parsing, wraps each cell to its column width (see [Multi-line cells](#multi-line-cells--wrapping-phase-f)), and records `row_display_rows` per source line. Per-cell:
   - `caret_in_cell` = any caret byte within the cell's trimmed `doc_range` (inclusive on both ends). When true, `resolve_cell_display` returns the **raw source bytes** so a partial formula like `=SUM(` doesn't render as `#ERR` mid-keystroke. When false, formula overrides apply normally.
   - Each `TableCellLayout` records `source_range: Range<usize>` (document-absolute trimmed-payload range) for downstream caret-in-cell tests at paint and hit-test time.
   - Column widths are quantized UP to the next multiple of `TABLE_COL_WIDTH_STEP_DIP = 16.0` so typing one character in a cell doesn't shift the column on every keystroke; the column only grows when content crosses the next step boundary. Reduces per-keystroke visual jitter AND chrome-cache invalidation (cache key includes col widths).

4. **`continuity_render::table_paint`** — D2D painter. For each `TableLayout` covering the current source line:
   1. `should_skip_alignment_row(layout, source_line)` — if the line matches `layout.alignment_row_source_line`, paint the alignment-row divider strip and return.
   2. Fill every cell rect with the body background — masks the underlying body glyphs left over after pipe hiding.
   3. Fill header-row cells with `markdown.table.header_bg`.
   4. Draw each cell's `display_text` aligned per column-alignment via `IDWriteTextLayout::SetTextAlignment`. Formula cells use `markdown.formula.value` / `markdown.formula.error` brushes; plain cells use the body foreground.
   5. Stroke a 1-DIP border (`markdown.table.border`) around every cell.

   Invoked two ways. **Focused pane:** `continuity_render::table_chrome_cache` records the *whole* table (every source row) into a per-table `ID2D1CommandList` before the renderer's outer `BeginDraw` and replays it with `DrawImage` after the body glyph pass via `renderer_table_chrome`. Cache key: `(document, block_start, layout_content_hash, theme_revision, font_state, dpi_scale, line_height, base_font_size)`. **Spectator panes:** `pane_body/table_chrome.rs` paints the chrome as a **post-pass after the body-text loop** — independent per-pane fonts/widths, no cache. Both size cell rects to the row's display-row count; stacking differs (see [Chrome stacking: focused vs spectator](#chrome-stacking-focused-vs-spectator)).

5. **Active-cell overlay** — `paint_active_cell_outline_line` paints **after** the chrome (cache replay for focused pane, inline for spectators) so the affordance sits on top of `body_bg`. Two states per cell:
   - **Cell-selected** (selection's ordered `(start, end)` matches `cell.source_range` exactly and is non-empty): 2-DIP outline + translucent fill at 25% alpha. No caret bar. Delete/Backspace/typing operate on the whole-cell selection via the normal text-edit path.
   - **Cell-editing** (a caret head lies within `cell.source_range`, selection doesn't fully cover): 2-DIP outline + 1-DIP vertical caret bar. Caret x positioned via `IDWriteTextLayout::HitTestTextPosition` for accurate placement under the actual font (monospace approximation is the fallback when layout build fails).

   `paint_focused_active_cell_outlines` iterates `selections × tables × cells` (NOT `tables × source_lines × cells`) to keep the per-frame cost O(carets × cells_per_table) regardless of row count. De-dups via a `painted: Vec<(layout_idx, cell_idx)>` so two carets in the same cell paint once.

6. **F4 swap gating** — `table_formula_paint::paint_table_overrides_line` / `_spec` skip any table whose `block_range` appears in `params.table_layouts`. Visual cells already render the evaluated value at the right x position; the byte-level swap painter would otherwise double-draw at the wrong x.

## Layout cache (Window-side)

`Window::last_focused_table_layouts: RefCell<HashMap<BufferId, Arc<Vec<TableLayout>>>>` — per-buffer cache of the most recent non-empty visual-table layouts for the focused pane.

- `build_focused_pane_table_layouts` returns `Arc<Vec<TableLayout>>`; both the cache insert AND the return are refcount bumps. `TableLayout` owns `String` cell text whose deep-clone would otherwise dominate per-keystroke paint cost on large tables.
- **Typing-lag is fixed at the decoration source, not papered over here.** While the decorate worker lags the rope by ≥1 revision (~30% of paint frames during a typing burst, per trace), the paint path consumes `Decorations::transformed_through`. That transform now remaps each table's `block_range` with **container semantics** (`continuity_text::transform_container_range_through`): an edit *interior* to the block range (i.e. typing in a cell) extends the range's end through the insertion instead of dropping the table. Previously the plain range transform dropped any span an edit overlapped, so the actively-edited table vanished from `evaluated_tables` every keystroke — both the display-map hide pass and the chrome painter key off `evaluated_tables`, so the table flickered between rendered cells and raw `| a | b |` markdown. Keeping the table alive lets `compute_table_layouts` produce a valid layout from the fresh rope each lag frame. A structural edit that straddles a block boundary, or a delete that collapses the whole range, still drops the table (so deleting a table doesn't leave ghost chrome).
- `Window::last_focused_table_layouts` cache fallback remains as a secondary guard for the residual case where `compute_table_layouts` returns empty against an in-bounds table (e.g. a multi-byte-char misalignment): the cached prior layout is reused so chrome paints continuously instead of dropping a frame.
- Mouse hit-test reads from this cache too (`try_cell_hit_at_pixel`), so single clicks still resolve to cells when the live layout is briefly empty.

## Click / double-click semantics

- **Single click in cell** → cell-selected state: selection set to `(cell.source_range.start, cell.source_range.end)` via `try_select_cell_at_pixel`. Empty cells (`source_range.start == source_range.end`) collapse to a caret at the click point.
- **Double click in cell** → cell-editing state: caret placed at click position (intra-cell x via the cached cell layout + `column_advance` monospace approximation in `cell_byte_at_pixel`). `select_word` is skipped inside cells.
- **Right click in cell** → context menu (Insert Row Above/Below, Insert Column Left/Right, Delete Row/Column/Table). Mirrors `try_tab_strip_context_menu` pattern. Caret moves to the clicked cell before menu items dispatch so structural ops target the right row/column.
- **Outside tables** — standard caret placement, word/line selection, no behavioral change.

## Keymap chord chain

Cell-scoped bindings (`when = "editor.in_table"`) that don't apply in the current sub-context return `CommandError::UnsupportedContext`. The chord dispatcher walks `Keymap::match_sequence_chain`, which yields every binding matching the chord in priority order; on `UnsupportedContext` (or `Skipped` — registry didn't resolve the command), it advances to the next-most-specific binding for the same chord. The chain terminates on `Handled` (success) or `Failed` (hard error) — neither of which retries.

Concretely:
- `markdown.table.move_up` at the table's top row returns `UnsupportedContext` → chain falls through to global `editor.move_caret_up` so the caret leaves the table normally.
- `markdown.table.enter` at column 0 of the table's first source line returns `UnsupportedContext` → chain falls through to global `editor.insert_newline`, pushing the table down by one line.
- `markdown.table.select_cell` on a pipe byte returns `UnsupportedContext` → chain falls through to global `editor.select_all`.

Without the chain, scoped no-ops are dead chords — the user would press the key and nothing would happen.

**Newline chords deliberately do NOT fall through.** `markdown.table.insert_break` (Ctrl+Enter) and `markdown.table.cell_up` (Shift+Enter) return `Ok` for every in-table miss rather than `UnsupportedContext`. Their global counterparts (`editor.insert_newline_below`, `editor.insert_newline`) insert a raw `\n` that splits the table across two source lines, breaking the pipe-table parse. Because `editor.in_table` and the handlers' own `focused_table` check agree, neither chord ever reaches its global fallback while the caret is in a table. (`markdown.table.enter` is the exception — it intentionally falls through at the table's first-line, column-0 edge to push the table down.)

## Cell-scoped keybindings

Bindings in `crates/keymap/assets/default.toml` with `when = "editor.in_table"`. The `editor.in_table` predicate scans `Decorations::evaluated_tables` for any table whose `block_range` contains the primary caret head; cheap, evaluated at dispatch time.

| Key | Command | Behavior |
|---|---|---|
| Tab | `markdown.table.tab_next` | Move caret to the next cell (left→right; wraps to the next non-delimiter row's first cell). At the last cell of the last body row, inserts a blank row below and lands the caret in its first cell. |
| Shift+Tab | `markdown.table.tab_prev` | Move caret to the previous cell (right→left). At the first cell of the first body row, no-op. |
| Enter | `markdown.table.enter` | Move caret to the cell directly below in the same column, skipping the alignment row. At the last body row, inserts a blank row and lands the caret in the new row's same column. |
| Shift+Enter | `markdown.table.cell_up` | Move caret to the cell directly above in the same column, skipping the alignment row — the inverse of Enter. At the header row (or any spot where the precise cell can't be resolved) it stays put. **Never** exits the table or inserts a raw newline: every miss returns `Ok` (no-op), so the chord can't fall through to the global `editor.insert_newline` that would split the table. |
| Ctrl+Enter | `markdown.table.insert_break` | Insert literal `<br>` at the caret. Phase F renders the `<br>` as a real in-cell line break (the cell wraps). Resolves the caret via `focused_table` (caret-anywhere-in-table), not `primary_cell_position`, so it fires from any position — on a pipe, a row edge, an empty cell — instead of falling through to the global Ctrl+Enter, whose raw newline split the table. |
| Up / Down | `markdown.table.move_up` / `_down` | Cell-row motion (same column, alignment row skipped). At the table's top/bottom edge returns `UnsupportedContext` so global Up/Down moves caret out of the table normally. Contrast Shift+Enter, which stays in the table at the top edge. |
| Ctrl+A | `markdown.table.select_cell` | Select whole cell content; carets outside cells return `UnsupportedContext` so keymap falls through to global select-all. |
| Home / End | `markdown.table.caret_cell_start` / `_end` | Jump to cell content edge. |
| Shift+Home / Shift+End | `markdown.table.extend_cell_start` / `_end` | Extend selection to cell edge. |

All handlers process selections per-caret: each caret picks its own behavior (cell-scoped vs default). When NO caret is in any cell, the handler returns `UnsupportedContext`.

## Commands

| CommandId | Bound to | Notes |
|---|---|---|
| `markdown.insert_table` | (palette) | Pre-existing. Inserts an N×M skeleton at caret line. |
| `markdown.table.insert_row_above` / `_below` | Right-click menu | One `EditOp::insert` of a blank `\| \| \| \|`-shaped row at the row above/below the caret's row. |
| `markdown.table.insert_col_left` / `_right` | Right-click menu | One `EditOp::insert` per row in descending byte order. Delimiter rows get `---\|`, other rows get `   \|`. |
| `markdown.table.delete_row` | Right-click menu | Refuses to delete the alignment row or the only body row. |
| `markdown.table.delete_col` | Right-click menu | Refuses when only one column remains. Deletes leftmost-pipe-through-content for col 0, otherwise content-through-trailing-pipe. |
| `markdown.table.delete_table` | Right-click menu | Replaces the entire `block_range` with empty. |
| `markdown.table.select_cell` | Ctrl+A | See above. |
| `markdown.table.caret_cell_start` / `_end` | Home / End | See above. |
| `markdown.table.extend_cell_start` / `_end` | Shift+Home / Shift+End | See above. |
| `markdown.table.tab_next` / `_prev` | Tab / Shift+Tab | Cell next/prev. Tab at last cell auto-inserts a row. |
| `markdown.table.enter` | Enter | Cell directly below; at last row, auto-inserts a row. |
| `markdown.table.cell_up` | Shift+Enter | Cell directly above (inverse of Enter). Stays at the header row; never exits the table or inserts a newline. |
| `markdown.table.insert_break` | Ctrl+Enter | Insert literal `<br>` at caret (renders as an in-cell line break). Resolves via `focused_table`, so it never falls through to a table-splitting newline. |
| `markdown.table.move_up` / `_down` | Up / Down | Cell above/below; falls through at edge. |

Structural ops are implemented inline on `Window` (not via `SelectionEdit`) because they need both the rope AND the decoration snapshot to locate the caret's cell. Each handler dispatches one or more `EditOp`s through `EditorHandle::apply_edit` for a single user-visible undo group. See `crates/ui/src/window_markdown_table_ops.rs`.

## Selection-suppressed unrender

A table renders as raw markdown (pipes + alignment row + formula source visible) when the active selection has reached **past a single cell** — Ctrl+A, Shift+arrow across rows, drag-select across cells, etc. The intra-cell editing UX (single-click cell selection, double-click edit, Tab navigation) is unaffected: a cell-selected state covers exactly the trimmed content range and never touches a pipe byte.

### Suppression predicate

A table is suppressed when **any selection's ordered (start, end) range covers at least one unescaped `|` byte inside the table's `block_range`**. Cheap O(selections × pipes-per-row) check; lives in `crates/render/src/table_suppress.rs::compute_suppressed_table_blocks` so the same logic feeds both the display-map hide pass and the render-side chrome painter.

Why pipes specifically? Cell-selected state is exactly a cell's trimmed content range — never includes a pipe byte. Any selection touching a pipe (including the alignment row, which is all pipes + dashes) is "wider than a cell" by construction. Caret-only (collapsed) selections never trigger; they can sit on a pipe without flipping the state.

### Pipeline integration

- **`continuity_display_map::table_hide_provider::compute_table_hidden_ranges_for_line`** — takes `suppressed_table_blocks: &[Range<usize>]` and emits no hides for tables whose `block_range` matches an entry. Raw `|`, `---`, and formula source bytes pass through to the body painter.
- **`continuity_display_map::segment_cache::compute_line_projection_stamp`** — hashes a per-table `is_suppressed` boolean derived from the same set. The per-line stamp changes when suppression flips for any covered table, busting `WrapCache` / `SegmentCache` entries so the next paint re-projects those lines with fresh hides.
- **`continuity_render::compute_table_layouts`** — same `suppressed_table_blocks` arg; skips building a `TableLayout` for matching tables. The F4 swap painter (`table_formula_paint`) still renders formula values inline at the source-byte position because it's already gated on `block_has_visual_layout` — when the layout is gone, the byte-level swap takes over.
- **`Window::compute_suppressed_table_blocks`** in `crates/ui/src/window_paint/payload.rs` computes the set once per paint from the active buffer's selections and decoration cache, then passes it to both `build_focused_pane_table_layouts` and `build_projection_request` so the worker rebuilds the display map with matching hides.

Per-pane: spectator paint computes its own set per pane (a Ctrl+A in pane A must not unrender the same buffer's table in pane B).

### Visual transition

Instant — no animation. Per the design principles, "chrome fade" is explicitly listed as instant. Selection-driven unrender flips frame-to-frame, matching the rest of the marker reveal pipeline.

### Failure modes

- Decorations transiently lag the rope by a revision across a multi-byte edit; the pipe-scan in `any_selection_covers_pipe` returns `false` rather than panicking on misaligned `byte_slice`. Suppression re-evaluates correctly on the next frame once decorations catch up.
- Selection ordering: the predicate always uses `Selection::ordered_range`, so reverse-direction selections (head before anchor) suppress the same way as forward.

## Caret-snap across hidden bytes

The display-map exposes every pipe as `Hidden`. Caret motion across these runs is handled by `Window::apply_structural_skip` in `crates/ui/src/window_link_clipboard.rs` — a post-step that runs after every command, advancing the caret while the next byte is structural, bounded by the current source line's byte length (no fixed iteration cap; long hidden runs like formula source payloads are skipped cleanly).

## DrawParams

```rust
pub table_overrides: &'a [EvaluatedTable],   // F4 byte-level swap (skipped when visual layout exists)
pub table_layouts:   &'a [TableLayout],      // visual cells (always rendered)
```

`MarkdownColors` carries the table colors:
```rust
pub table_border: Rgba,      // markdown.table.border
pub table_header_bg: Rgba,   // markdown.table.header_bg
pub table_alignment_bg: Rgba,// markdown.table.alignment_bg
```

The active-cell outline is themed by the dedicated `markdown.table.active_cell_outline` token (both the focused and spectator paths). The translucent "cell selected" fill is the same color at reduced opacity (`ACTIVE_CELL_SELECTED_FILL_ALPHA`), so the outline and fill read together. The in-cell caret bar keeps the editor caret color (`editor.cursor.primary`) so it matches the body caret. Bundled themes default the new token to their caret color, preserving the prior look. The outline/fill/caret all paint **fresh each frame on top of the cached chrome** (caret position is per-frame), so this token never affects the chrome cache beyond its existing `theme_revision` key — and never touches row counts or display-map layout.

## Layout invariants

- Tables render **unconditionally**. Pipes are always hidden in the display map; visual chrome paints over every table block regardless of caret position.
- Column widths in DIPs are constant for the whole table and quantized to a 16-DIP step; cell rects stack vertically. A row's cell rect spans `row_height` display rows (1 for an ordinary row, N for a wrapped / `<br>` row); rows below it shift down by the reserved height. The chrome paints once per row at its first display row — the focused path stacks by cumulative `display_row_offset_within_table`, the spectator post-pass anchors each row at the frame's actual `first_display_line_index_for_source`.
- Coordinates are **layout-local**: the painter operates in `[0, total_width_dip] × [0, line_height_dip]`. In the spectator path the per-line `SetTransform` places body origin at `(0, 0)`; in the focused-pane path the cache recorder installs a per-row `SetTransform` and the replay transform aligns table-local `(0, 0)` with `(body_origin.0 + margins.left, body_origin.1 + first_display_row * line_height - scroll_y)`.
- Cell content text shown while the caret edits the cell IS the raw source bytes (wrapped byte-preservingly to the column width — see [Multi-line cells](#multi-line-cells--wrapping-phase-f)); formula evaluation is suppressed per-cell while a caret falls within `cell.doc_range`. Other cells in the same row/table keep their evaluated values throughout.

## Tests

- `crates/display_map/src/table_hide_provider.rs::tests` — unit tests covering empty input, unconditional hiding (caret position no longer affects hides), alignment-row hidden, formula payload hidden, multi-table hiding, escaped pipes, plain-text lines.
- `crates/render/src/table_layout.rs::tests` — unit tests covering layout-always-built (caret-agnostic), alignment parsing (Left/Center/Right), formula eval value rendering, min-width floor, wide-content width growth with quantization, alignment-row empty text, `cell_x_dip` math, `covers_source_line`, source-line cell filter, `alignment_row_source_line` population, and caret-in-cell wrapping while editing (long content wraps, row reserves >1 display row, raw markers kept).
- `crates/render/src/table_layout/cell_wrap.rs::tests` — `<br>` split/trim, greedy word-wrap, long-token char-break, and byte-preserving editing wrap (`wrap_raw_preserving` reproduces the source, keeps markers literal, clip mode one line).
- `crates/render/src/pane_body/table_chrome.rs::tests` — spectator chrome tiles contiguously over the frame's reserved rows, and spans the frame's full row count when a raw table line soft-wraps beyond the cell-wrap reservation.
- `crates/render/src/table_paint.rs::tests` — unit tests covering `block_has_visual_layout` (F4 swap gating) and `should_skip_alignment_row`.
- `crates/render/tests/pixel_canary.rs` — table fixtures lock the visual chrome via blake3 hashes of the rendered back buffer.

## Key files

- decorate (formula eval): `crates/decorate/src/table_eval.rs`, `table_eval/chain.rs`, `table_formula.rs`, `table_formula_parser.rs`
- display-map (pipe hide): `crates/display_map/src/table_hide_provider.rs`
- render (layout): `crates/render/src/table_layout.rs`, `table_layout/build.rs`, `table_layout/parse_row.rs`, `table_layout/cell_inline.rs`, `table_layout/cell_wrap.rs` (wrap + `<br>` split), `table_layout/directive.rs` (`<!--continuity:…-->`)
- render (paint): `crates/render/src/table_paint.rs`, `table_paint/active_cell.rs`, `table_formula_paint.rs`, `table_chrome_cache.rs`, `renderer_table_chrome.rs`, `pane_body/table_chrome.rs` (spectator chrome post-pass)
- ui (cell hit-test): `crates/ui/src/window_mouse_hit_test.rs::try_cell_hit_at_pixel` / `try_select_cell_at_pixel`
- ui (click dispatch): `crates/ui/src/window_mouse.rs::on_left_button_down` / `on_left_button_dbl`
- ui (right-click): `crates/ui/src/window_context_menu.rs::try_table_cell_context_menu`
- ui (structural ops): `crates/ui/src/window_markdown_table_ops.rs`
- ui (pasted-table block normalization): `crates/ui/src/window_markdown_table_ops/paste_normalize.rs`
- ui (cell-scope nav: Tab / Enter / Shift+Enter / Ctrl+Enter / Up / Down): `crates/ui/src/window_markdown_table_nav.rs`
- ui (`editor.in_table` predicate): `crates/ui/src/window_commanding/context.rs`
- commands: `crates/command/src/markdown.rs`
- keymap: `crates/keymap/assets/default.toml`

## Inline styling inside cells (Phase B)

Each `TableCellLayout` carries `inline_runs: Vec<(Range<u32>, SpanStyle)>` — UTF-8 byte ranges indexing into `display_text` plus a `SpanStyle` (`bold`, `italic`, `strikethrough`, `underline`, `role`). The painter (`apply_cell_inline_runs` in `table_paint.rs`) translates each byte range to UTF-16 code-unit indices and applies `SetFontWeight` / `SetFontStyle` / `SetStrikethrough` / `SetUnderline` per run.

The parser (`crates/render/src/table_layout/cell_inline.rs::compute_cell_inline`) is a narrow inline scanner — single-level bold (`**…**` / `__…__`), italic (`*…*` / `_…_`), inline code (`` `…` ``), strike (`~~…~~`), and `[text](url)` links. Markers strip from `display_text`; only the inner content shows. Nested bold-inside-italic is supported recursively. Unmatched markers render as literal characters. Footnote refs, image refs, and per-line list-marker continuation are NOT scanned in cells (rare in tables, parser stays small).

Cells in the formula-evaluator override path (`is_formula = true`) skip the inline scanner — their `display_text` is the evaluated numeric value. The alignment row (`is_alignment_row = true`) carries empty text and no inline runs. **Caret-in-cell cells keep raw source bytes** as `display_text` with empty `inline_runs`, so the user can edit `**` markers as ordinary text; the painter then renders the literal characters and the active-cell outline marks the cell.

**Column-width stability across caret transitions** — column widths are sized from a `measurement_text` (always the markers-stripped form, regardless of caret position) so clicking into a cell with markers does NOT change column widths or invalidate the chrome cache. Without this gate, every caret enter/exit on a markup-bearing cell would jiggle columns and burst the per-frame chrome cache.

## Multi-line cells + wrapping (Phase F)

A table row can occupy **more than one display row**. A cell wraps when its content exceeds the column's inner width, or carries a `<br>` hard break. The row's display height is the tallest cell on that source line; every other row stays 1 row. Built in `crates/render/src/table_layout/cell_wrap.rs`; rendered by `table_paint` stacking each cell's `lines: Vec<CellLine>` top-to-bottom.

### Presentation directive

An optional `<!--continuity:width=<col widths>;wrap=on|off-->` comment on the line **immediately above** the table controls column widths and the wrap mode. Parsed by `table_layout/directive.rs`; the line is hidden from the display map (zero display rows, like a marker) but kept in the rope as the source of truth. `width=-,110,164` sets explicit DIP widths per column (`-` = auto-size); column borders are drag-resizable (a live drag previews via `TableColWidthOverride`, then rewrites the directive on release). `wrap=off` clips over-wide content to the column edge instead of wrapping. Absent directive ⇒ auto widths, wrap on.

### Layout build (two passes)

`compute_table_layouts(tables, rope, caret_bytes, suppressed_table_blocks, measure)` → `Vec<TableLayout>`. Pass 1 parses cells and `<br>`-segments; pass 2 (after the capped column widths are known) wraps each cell to its inner width and records `TableLayout.row_display_rows: Vec<u32>` (display rows per source line, indexed by `source_line - first_source_line`). Column auto-size is capped at `DEFAULT_TABLE_COL_WIDTH_MAX_DIP` (220) so prose cells wrap rather than stretching the pane; an explicit directive/drag width may exceed it up to `MAX_TABLE_COL_WIDTH_DIP` (800).

### Display-map reservations

`table_row_reservations(layouts)` emits one `ImageRowReservation` per source line whose `row_display_rows > 1` (reusing the inline-image phantom-row path). The UI merges these with the image reservations (`window_image_placements::merge_table_row_reservations`, max per source line) before building the `FrameDisplay`, so a tall table row reserves its extra display rows and body / gutter / caret below it stay aligned. The reserved rows are `is_wrap_continuation` phantoms (no glyphs); the painter draws chrome only on the row's first display row, spanning the full reserved height. **The 1:1 source↔display claim above holds only for single-row tables;** a wrapped/`<br>` row breaks it by design, and the reservation keeps everything below aligned.

### Wrapping while editing

A caret-in-cell still shows **raw source** (markers + literal `<br>` visible, so typing is WYSIWYG) but now wraps it byte-preservingly (`cell_wrap.rs::wrap_raw_preserving`) instead of clipping to one line. The wrapped lines concatenate back to the source exactly (break spaces stay at line ends), so the in-cell caret bar maps a source-byte caret to its wrapped row via cumulative line byte lengths (`active_cell.rs::locate_caret_line`). This keeps the row's display height roughly stable across caret enter/exit (rendered-wrapped ≈ editing-wrapped) instead of collapsing the cell to one line — fewer layout shifts, per the caret-line-screen-y principle.

### Chrome stacking: focused vs spectator

Both paths size each cell rect to the row's display-row count, but anchor differently:
- **Focused pane** — `table_chrome_cache::record_table_chrome` stacks rows by `TableLayout::display_row_offset_within_table(row)` (cumulative `row_height`), recorded once into the command list and replayed at the table's first display row.
- **Spectator panes** — `pane_body/table_chrome.rs::paint_spectator_table_chrome` runs as a **post-pass after the body-text loop** (mirroring the focused command-list replay), and anchors each row at the frame's *actual* `first_display_line_index_for_source(row)`, spanning the frame's *actual* `display_line_count_for_source(row)`. Following the frame (not `row_height`) keeps the chrome tiled exactly over the projected rows even when a promoted focused frame (after a focus switch) or a soft-wrapped raw table line allocates a source line a different row count than the cell-wrap reservation implies. An inline per-row spectator paint (the pre-fix shape) masked only a tall row's first display row, leaving the wrap-continuation glyphs bleeding over the cell grid.

## Pasted-table block normalization

A pasted GFM pipe table (from `Ctrl+V` — including a `CF_HTML` fragment converted to markdown; see [Clipboard](clipboard.md)) is normalized so tree-sitter-md classifies it as a `PipeTable`. GFM requires a pipe table to **begin a block**: when a table is inserted directly after a non-blank line with no preceding blank line, tree-sitter-md folds the header into the preceding paragraph and the snippet never becomes a `PipeTable` — so the visual-cell pipeline never engages and the user sees raw `| a | b |` pipes forever.

The paste path (`Window::insert_paste_text` → `normalize_table_paste` in `crates/ui/src/window_clipboard.rs`) calls the pure helpers in `crates/ui/src/window_markdown_table_ops/paste_normalize.rs`:

- **Detection** — `is_gfm_table_text` (header pipe row + delimiter row) or `is_pipe_table_missing_delimiter` (≥2-column header pipe row followed by a non-delimiter pipe row). Non-table pastes pass through unchanged. Conservative: a lone `|`-prefixed line and a single-column body are left alone (avoids treating incidental prose as a table).
- **Leading-newline prefix** — when the primary caret is NOT at column 0 of a blank line (`primary_caret_at_blank_line_start`), `normalize_pasted_table` prefixes a `\n` so the table starts its own block. At column 0 of a blank line (or an empty buffer) no prefix is added.
- **Missing-delimiter synthesis** — when the snippet looks like a table body that lost its delimiter row, `insert_missing_delimiter_row` inserts a `| --- | --- |` row (column count from `pipe_table_column_count`, escaped pipes not counted) immediately after the header.

Reuses `super::is_delimiter_line` so the paste path agrees with the table-ops parser on what a delimiter row is.

**Plain paste (`Ctrl+Shift+V`) bypasses this** — `insert_plain_clipboard_text` does not run `normalize_table_paste`, so a table pasted as plain text keeps its raw shape.

### One-frame raw flash

Immediately after the paste lands, the table can render as raw `| a | b |` for a single frame before the decorate worker reparses and `evaluated_tables` populates. This is decoration lag (the chrome painter keys off `evaluated_tables`), not a normalization bug — no code fix; it resolves on the next paint.

### Tests

- `crates/ui/src/window_markdown_table_ops/paste_normalize.rs::tests` — full-table vs missing-delimiter detection, lone-header / single-column conservatism, column counting (escaped pipe excluded), delimiter-row format, newline prefix gated on block-start, and combined synthesis + prefix.

## Out of scope (deferred)

- **Cell-rect selection model** — multi-cell selections (drag across cells) would need a new `Selection::Kind::Cell` variant. Today selection remains contiguous bytes; spreadsheet-style block selection across non-contiguous cells is not modeled.
- **Multi-line inline styling** — a wrapped sub-line renders plain; inline markdown styling (`**bold**`, links) rides through only on a cell line that is an unwrapped whole `<br>`-segment. Wrapping style runs across break points needs per-line range re-slicing.
- **Cross-table formula references** — `CellRef` is intra-block; the F4 formula language doesn't span tables.
