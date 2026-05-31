# Decoration

Pure function `(RopeSnapshot, Revision) → Decorations`, run on a worker pool. Tree-sitter-md produces block + inline span data for headings, sections, autolinks, rainbow brackets, and syntax-highlighted code fences. Stale results (revision mismatch on arrival) are silently dropped.

## What it is
- Pure function `(RopeSnapshot, Revision) -> Decorations`. Runs on a worker pool, never mutates. Tree-sitter incremental parsing produces block + inline span data; pure helpers walk the tree for headings, sections, rainbow brackets, autolinks. Stale results (revision mismatch) are dropped on arrival. A watchdog restarts any decoration worker that stops reporting progress beyond the configured timeout.

## Key concepts
- **`Decorations`** — aggregate of: block spans (paragraphs, headings, fences, lists), inline spans (emphasis, code, links, image refs, checkboxes, markers, footnote references/definitions), heading entries, table alignments, syntax highlights, autolink ranges, rainbow bracket depths, **inline color/highlight spans (Phase F3)**, **per-frame table-formula evaluations (Phase F4)**.
- **`BlockSpan { kind, start_byte, end_byte }`** — `BlockKind::Heading{level}`, `SetextHeading`, `Paragraph`, `FencedCodeBlock`, `IndentedCodeBlock`, `Blockquote`, `List`, `ListItem`, `Table`, `ThematicBreak`.
- **`InlineSpan { range, kind }`** — `InlineKind::Strong | Emphasis | Strikethrough | Code | Link{text_range, url_range} | FootnoteReference{label} | FootnoteDefinition{label, body_range} | ImageRef{alt_range, url_range} | Marker(MarkerKind) | Checkbox{toggle_byte, checked}`.
- **`MarkerKind`** — `HeadingHash`, `FenceTick`, `BlockquoteCaret`, `EmphasisDelim`, `StrikeDelim`, `CodeDelim`, `ThematicBreak`, `ListMarker`, `TablePipe`.
- **`HeadingEntry { line, level, text_range }`** — drives breadcrumb (F1), outline sidebar (F2), section commands (H4), goto-heading.
- **`SectionBounds { start_byte, end_byte }`** — Phase A6 driver for promote/demote/move section, slash-command context.
- **`AutoLink { range, kind }`** — Phase B12 bare-URL detector.
- **`BracketDepth { byte, depth, opening }`** — Phase B8 rainbow bracket palette index source.
- **`InlineColorSpan { outer, inner, kind }`** — Phase F3. `kind = Highlight` for `==text==` (theme-resolved highlight color) or `Hex(rgba)` for `{#rrggbb:text}` (3 / 4 / 6 / 8 hex digits, packed `0xRRGGBBAA`). Spans never cross newlines; outermost delimited region wins for adjacent / nested markup; empty `====` / `{#hex:}` is not a span. Source bytes stay literal — only the renderer paints the run with the resolved color and hides delimiters per the display-map hide pass.
- **`EvaluatedTable { block_range, overrides }`** + **`TableCellOverride { cell, cell_range, display }`** — Phase F4. Per pipe-table block, every cell whose trimmed text starts with `=` is parsed through `decorate::table_formula::parse_formula` and run against the block's value matrix (`Vec<Vec<Option<f64>>>`). The result becomes the cell's render-time display text; the source bytes stay byte-exact markdown. Errors render as `#DIV/0!` (divide-by-zero) or `#ERR` (other `FormulaError` variants). The block-level reveal predicate is `Decorations::caret_inside_any_table_block(caret_bytes)` — when true, the renderer shows source formulas back so the user can edit them.
- **Footnotes** — `inline_text::scan_text_inlines` emits `FootnoteReference` for body `[^label]` spans and deliberately skips definition labels. `footnotes::footnote_definition_spans` runs a whole-document pass for `[^label]: body` definitions, including indented continuation lines, so hover-peek and Ctrl+click navigation can resolve `label -> body_range` without reparsing in the UI.
- **Worker watchdog** — `WorkerWatchdog` records each worker generation's last-progress timestamp. A timeout retires the hung generation, detaches its thread handle so shutdown cannot block on it, requeues the in-flight request when one exists, and starts a fresh worker generation. Stale late results are rejected by generation before they can enter the result channel.

## Data model

```rs
struct Decorations {
    revision: Revision,
    blocks:    Vec<BlockSpan>,
    inlines:   Vec<InlineSpan>,
    headings:  Vec<HeadingEntry>,
    highlights:Vec<HighlightSpan>,    // syntax highlights for code blocks
    tables:    Vec<TableAlignment>,
}
```

Pure helper functions:
- `headings(tree, source) -> Vec<HeadingEntry>` (`decorate::headings`).
- `sections::heading_at/heading_chain_at/section_bounds(headings, byte, source_len)` (Phase A6).
- `auto_links(text) -> Vec<AutoLink>` (Phase B12).
- `bracket_depths(text) -> Vec<BracketDepth>` (Phase B8).
- `block_inline_spans(tree, source) -> Vec<InlineSpan>`.
- `footnote_definition_spans(source) -> Vec<InlineSpan>`.
- `block_spans(tree) -> Vec<BlockSpan>`.
- `column_alignments(tree, source) -> Vec<TableAlignment>`.
- `highlight(text, language) -> Vec<HighlightSpan>` — `Rust`, `Json`, `Markdown`, falls back to plain for unknown.

## Operations
- **Worker pool**: `DecoratePool::spawn(num_workers)` produces a fixed-size `crossbeam` worker pool. UI submits `DecorateRequest { buffer_id, rope: Arc<Rope>, revision, prev_revision, deltas_since_prev, full_parse_reason }`; the worker materializes the surviving latest-wins request to `String` exactly once before parse/extract. Pool emits `DecorateResult` with the resulting `Decorations` stamped with the request's revision plus parse-path trace metadata. UI merges through `DecorationCache`, whose revision-monotonic insert rejects regressions.
- **Watchdog restart**: `DecoratePool::spawn_with_watchdog_timeout(num_workers, timeout)` installs the same pool with a configurable watchdog. `DecoratePool::worker_restarts()` exposes `DecorateWorkerRestart` events; the UI drains them, re-submits visible decoration work, and shows a non-modal one-shot status-bar chip.
- **Per-buffer cache**: `DecorationCache` keyed by `BufferId` holds the last-accepted `Arc<Decorations>` per buffer on the UI thread. `get()` returns `&Decorations` for read-only consumers; `get_arc()` returns `&Arc<Decorations>` for hot paths that need an owned handle without deep-cloning spans. Decoration workers separately own `BufferTreeCache`, a bounded LRU of cached tree-sitter trees keyed by buffer with revision and source length stamps.
- **Incremental parse**: `Decorations::compute_incremental` applies every point-augmented rope delta to the cached tree in order, then calls `MarkdownParser::parse(text, Some(&edited_tree))` once. The worker uses this path only when the producer's `prev_revision` matches a cached tree and the cached source length plus delta shifts equals the new source length; otherwise it full-reparses.
- **Trace**: worker results carry `DecorationParseTrace`. `window_decoration.rs` emits `event:decoration_parse_incremental buffer=<id> ranges=<count> elapsed_us=<us> cached_source_len=<bytes>`, or `event:decoration_parse_full buffer=<id> reason=no_prev_tree|covered_false|sanity_check_failed elapsed_us=<us>`.
- **Language detection**: `detect(rope_first_lines) -> Language` (`Markdown` | `Plain` | `Rust` | `Json`). Determines which parser runs and powers the `language` context atom.

## API surface
- Public re-exports from `crates/decorate/src/lib.rs`:
  - `MarkdownParser`, `Decorations`, `BlockSpan`, `BlockKind`, `InlineSpan`, `InlineKind`, `MarkerKind`, `HeadingEntry`, `SectionBounds`, `HighlightSpan`, `HighlightKind`, `TableAlignment`, `AutoLink`, `AutoLinkKind`, `BracketDepth`.
  - Functions: `headings`, `block_spans`, `block_inline_spans`, `footnote_definition_spans`, `auto_links`, `bracket_ranges`, `bracket_depths`, `column_alignments`, `highlight`, `detect`, `heading_at`, `heading_chain_at`, `heading_index_at`, `section_at`, `section_bounds`.
- Pool / cache: `DecoratePool`, `DecorateRequest`, `DecorateResult`, `DecorateWorkerRestart`, `PoolShutdown`, `DecorationCache`, `DEFAULT_WORKER_WATCHDOG_TIMEOUT`.

## Configuration
- `markdown.reveal_mode` = `"block"` | `"line"` — controls which marker spans show as `Hidden` vs `Revealed` in the display map.
- Heading scales / colors are theme-driven (see `theme.md`).
- `workers.decoration_watchdog_ms` defaults to `2000` and validates to `100..=600000`. Hot reload updates the live `DecoratePool` timeout. Pool size is still fixed at startup.

## Key files
- pool: `crates/decorate/src/pool.rs`
- worker watchdog: `crates/decorate/src/worker_watchdog.rs`
- cache: `crates/decorate/src/cache.rs`
- parser: `crates/decorate/src/parser.rs`
- spans: `crates/decorate/src/spans.rs`, `src/inline.rs`, `src/inline_text.rs`
- footnotes: `crates/decorate/src/footnotes.rs`, resolver helpers in `src/decorations.rs`
- headings + sections: `crates/decorate/src/headings.rs`, `src/sections.rs`
- syntax highlights: `crates/decorate/src/syntax.rs`
- tables: `crates/decorate/src/tables.rs`
- autolink (B12): `crates/decorate/src/autolink.rs`
- rainbow brackets (B8): `crates/decorate/src/rainbow.rs`
- language detect: `crates/decorate/src/language.rs`
- aggregate: `crates/decorate/src/decorations.rs`

## Fold semantics (§H3)

Two fold providers feed the same per-window `PaneModesState.folded_lines: Vec<u32>` set:

- **Indent fold** — `continuity_core::compute_indent_fold_byte_ranges(rope, &folded_lines)`. For each toggled line, computes the indent-subtree end via `edit_indent_subtree::indent_subtree`; emits a `IndentFoldByteRange` covering the body lines.
- **Heading fold** — `continuity_core::compute_heading_fold_byte_ranges(rope, &[(line, level)], &folded_lines)`. For each toggled line that matches a heading, the body runs to the next heading at the same-or-shallower level (or EOF).

`crates/ui/src/window_paint.rs::on_paint` calls both providers per frame, concatenates the outputs, then sort-coalesces overlapping ranges. Headings naturally take priority on conflict — the coalesce step extends overlapping ranges to the larger end, and heading folds are typically larger.

The gutter painter (`crates/render/src/chrome_fold.rs::compute_fold_headers`) unifies both fold kinds when computing the gutter `▸ N` indicator. A line is foldable if **either** its indent subtree has a deeper body **or** it appears in the markdown heading list — see `is_line_foldable_with_headings`.

Sentinel `u32::MAX` (fold-all) expands to every top-level indent subtree **and** every heading at the shallowest level present (H1 if any, else the smallest level number that appears).

Persistence reuses the existing `folded_lines` plumbing in `pane_tree_codec` (`#[serde(default)]` field, no schema change). Both fold kinds are restored on window open via the same `install_restored_folded_lines` validator.

## Heading-size predicate

The heading font scale is gated on the *projection's hide state*, not
the block kind. The display map applies `SpanStyle::heading(level)`
only when the same `line_revealed(decorations, caret_bytes, line_start,
line_end)` predicate that hides `MarkerKind::HeadingHash` returns
false. A heading line whose marker bytes are revealed (caret inside
the block in `reveal_mode = "block"`, or caret on the same line in
`reveal_mode = "line"`) renders at `SpanStyle::heading_revealed`
— body font scale — so the `#` markers don't reflow the line. Painter,
width measurer, and mouse hit-test all resolve the scale from
`SpanStyle::font_scale` rather than only from `SpanRole::Heading`, so
the three coordinate systems agree on glyph width during the reveal.

## Block decoration paint (P-thematic-break fix)

`paint_block_backgrounds` and `paint_horizontal_rules` translate
source-line → display-row through `FrameDisplay` before computing Y.
Both helpers take `frame_display: &FrameDisplay`; multi-line panels
cover the inclusive display-row range. Soft-wrap continuation rows
above a fenced-code panel, blockquote bar, or horizontal rule push
the painted divider down by the same total — without the helpers,
the painter would drift away from glyphs whenever a wrapped paragraph
sat above the block. A fully folded source line returns no row and
the painter skips it. The caret-inside-block reveal predicate
(operating on source lines) is unchanged. See `paint-flow.md` §
"Block-decoration painters".

## Inline code distinctness + fenced-block copy affordance

Inline `` `code` `` spans paint with a subtle `markdown.code.background`
fill plus `INLINE_CODE_BG_PAD_DIP = 2.0` horizontal padding in both the
soft-wrap and no-wrap paths. The soft-wrap painter
(`renderer_line_text_pass::wrap_paint`) additionally publishes per-frame
client-DIP hit rects for hover detection; the no-wrap path paints the
background only.

Fenced code blocks gain a hover-driven copy affordance. UI tracks one
`CodeCopyHover` per `MouseState`; the renderer paints
`CodeCopyButtonDraw` after the scrollbar / line-number gutter and
before overlays. The button hides at paint time if any caret sits
inside the block — using the same `block_revealed` predicate as the
fence-tick marker reveal — so button visibility and fence-tick visibility
cannot disagree. Clipboard write reads the live rope via
`fenced_inner_text` rather than cached decoration metadata. Block
highlight starts at `body_text_left_dip` (matches first body glyph
column) and ends at `fenced_block_right_edge` —
`longest_line_chars × column_advance + FENCED_BLOCK_RIGHT_PADDING_DIP`
(12 DIP) capped by content width — so short blocks no longer paint a
viewport-wide band. Copy feedback persists for
`CODE_COPY_FEEDBACK_TIMER_MS = 1500` ms; the Copied background tints
the active theme's caret color at α=0.92, with a luma-threshold-picked
foreground glyph so the checkmark stays legible across themes.

The inline copy button is soft-wrap-only in v1 (no-wrap mode paints the
distinct background but does not publish hover hits). Non-focused panes
do not paint inline-code background. Trace event:
`event:code_copy kind=fenced|inline chars=N language=<str>`.

Theme reuse: no new keys. `markdown.code.background`,
`markdown.code_block.background`, `markdown.code_block.border`, and
`markdown.code.foreground` all carry 5/5 bundled coverage and drive the
affordance; feedback tints derive at paint time by RGB blend toward the
caret accent.

## Stale-parse re-labeling and parse-revision tracking

`Decorations::transformed_through(deltas, new_revision)` walks every span's byte range through a chain of rope deltas (spans entirely before any edit stay put; spans entirely after shift by the accumulated byte delta; spans intersecting any edit are dropped) and **re-labels** the result with `new_revision`. The new revision is whatever rope revision the caller wants the transformed decoration to apply to — typically the current rope rev at paint time.

Consequence: `Decorations::revision` is an "applies-to" label, not a "parsed-at" identifier. Two `Decorations` snapshots can share the same `revision` while their underlying parse content differs — e.g. a stale-transformed prior parse and a fresh worker parse delivered against the same rope rev.

The UI side compensates with `Window::last_painted_decoration_parse_revision: Option<u64>`, sampled from `decoration_cache.get(id).revision` **before** the transform applies. The projection classifier's covering-cache fast path consults this via `ProjectionClassifyInputs::decoration_parse_advanced` and falls through to a rebuild when the parse rev has advanced — see `display-map.md` § dirty-set spill + decoration parse-revision invalidation.

## Relates to
- [Buffer](buffer.md) — workers consume `RopeSnapshot`.
- [Display map](display-map.md) — `Decorations` is the source of `Hide` / `Replace` actions that produce the display projection.
- [Rendering](rendering.md) — block styles + syntax highlights bake into `IDWriteTextLayout` attributes.
- [Overlays](overlays.md) — headings drive goto-heading; sections drive H4 outline manipulation.
