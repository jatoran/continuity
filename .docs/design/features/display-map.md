# Display map

Pure projection from source bytes to visible display rows: hide marker bytes (e.g. `==highlight==` markers, table pipes, table formula source bytes, table alignment-row content), replace **unordered** list markers with `• ` (ordered `N.` / `N)` markers keep their literal number), collapse folds, break lines at soft-wrap points. The rope stays canonical; the display map is derived — torn out, the editor is degraded but correct.

## What it is
- Pure projection from source bytes to a visible display string per source line. Hides marker bytes, replaces unordered list markers with `• ` (ordered markers keep their number), collapses folds, breaks lines at soft-wrap points. Source positions stay canonical for undo/persistence/search/file-I/O; the display map is *derived* — removing it yields a degraded but correct editor.

Phase 17.5 charter: the `IDWriteTextLayout` never holds source bytes that aren't supposed to be visible. Reveal is structural, not painted-over; the legacy `paint_hidden_marker_rects` and `apply_line_decorations` paths no longer exist.

## Key concepts
- **`SourceByte` / `DisplayByte` / `DisplayUtf16` / `SourceLine`** — typed coordinate-space newtypes. Mixing them is a compile error.
- **`DisplayMap`** — immutable `Arc` snapshot of every visible display line plus source↔display byte tables. Built by `FrameDisplay::build(rope, revision, Option<&Decorations>, &[caret_byte], wrap_width, char_width)`.
- **`DisplayLineSpec`** — one visible line. Carries `segments: Vec<DisplaySegment>`, the baked `display_text: Box<str>`, source-to-display tables, and a content stamp the renderer keys the `IDWriteTextLayout` cache against.
- **`DisplaySegment`** — `Visible { source, style, hit }` | `Hidden { source }` | `Replace { source, display, style, hit }`. Concatenating each segment's `display_bytes` yields the line's `display_text`.
- **`SegmentHit`** — `None | Checkbox { toggle, checked } | Link { url } | FootnoteReference { label, definition } | FootnoteDefinition { label, first_reference }`. Drives click → toggle / link-follow / footnote-jump without re-parsing. A `Link` whose target starts with `#` is an in-document anchor: the click resolves the GitHub-style heading slug against the buffer's ATX headings and jumps the caret there instead of shelling out (`ui::window_mouse_segment_hit::jump_to_heading_anchor`).
- **`SpanStyle`** — typed display-time style: `role`, `bold`, `italic`, `strike`, `font_scale_q8`, etc. Cheaper than carrying decoration spans through paint.

## Pipeline

```
build_line_segments(decorations, caret_bytes, folds, line_start, line_end, line_text)
  ├─ folds         → Action::Hide spans
  ├─ inline spans  → Action::Hide / Action::Replace depending on reveal_mode + caret
  ├─ checkboxes    → Action::Replace with `☐ ` / `☑ ` + SegmentHit::Checkbox
  ├─ links         → Action::Replace with the visible text run + SegmentHit::Link
  ├─ pipe tables   → Action::Hide pipes, delimiter rows, and formula source in visual mode
  ├─ footnotes     → Action::Replace `[^N]` with superscript `N`, unless caret is inside
  ├─ list markers  → Action::Replace with `• `
  ├─ snap action boundaries to char boundaries (defensive: stale spans during undo)
  └─ coalesce_segments → Vec<DisplaySegment>
DisplayLineSpec::new(…) → bake display_text, source_to_display, display_to_source,
                          display_byte_to_utf16, content_stamp
soft_wrap_spec(spec, line_text, wrap, measure) → Vec<DisplayLineSpec>
                          # one source line may map to N display lines when wrap fires
```

Source ↔ display invariants:
- For `Visible` segments, `display_bytes().len() == source.end - source.start` (post char-boundary snap).
- For `Replace` segments, `display_bytes().len()` = the replacement string length.
- Sum of `display_bytes().len()` across a line's segments = `display_text.len()` = `display_byte_to_utf16.len() - 1`. The `style_runs` cursor math depends on this; a producer that violates it would trip the line's `assert_sorted_and_contiguous`.

## Operations
- **Build**: `FrameDisplay::build(rope, revision, decorations, caret_bytes, wrap_width_dip, char_width)` produces a full `Arc<DisplayMap>`. Paint normally goes through the classifier in `ui::window_projection_plan` so it can reuse a cached frame, rebuild a dirty/spliced subset, realize only the viewport, or run a partial row-index walk.
- **Build with folds** (§H3): `FrameDisplay::build_with_folds(..., folds: &[FoldRange], ...)` accepts user-toggled fold ranges. Empty `folds` is equivalent to `build`. Folds are computed from `PaneModesState.folded_lines` by `continuity_core::compute_indent_fold_byte_ranges` (the provider lives in `core` because `display_map` does not depend on it; the wrap into the `FoldRange` newtype happens at the ui call site).
- **Build with options** (γ): `FrameDisplay::build_with_options(..., folds, image_reservations: &[ImageRowReservation], ...)` is the superset entry point that additionally injects phantom display rows for expanded inline images. Reservations are produced by `image_row_reservation_provider::compute_image_row_reservations` from per-frame inline-image placements + a peek at the renderer's `ImageCache` for native dimensions; collapsed and cold-cache placements emit no entry. See `features/image-paste.md` § Row reservation.
- **Progressive row-index walk**: `DisplayMapBuilder::compute_partial_row_index_for_viewport_with_stats` and its dirty/splice siblings walk the viewport source range plus a safety margin first, store exact row counts for that range, and leave offscreen rows as one-row placeholders tagged by `PartialRowIndexState`. UI paint uses these paths for large cold, dirty, and splice variants; post-paint worker dispatch fills the full index when useful.
- **MRU prewarm** (Phase β): `ui::display_prewarm_cache::DisplayMapPrewarm` keeps a bounded UI-thread queue of `(BufferId, PrewarmStage)` for the focused pane's next two MRU buffers. Stages run caret → viewport → decoration. The cache key (`PrewarmQuery`) is `(buffer, rope_revision, decoration_revision, caret_bytes, folds, image_reservations, wrap_width, font_state)`, and `Window::on_paint` asks it for an exact `FrameDisplay` before cold-building one, so a prewarmed Ctrl+Tab target feeds the renderer directly. The idle prewarm itself still builds reservation-free (`&[]`) frames, so it only ever serves the no-image path; a buffer with expanded images relies on the focused last-paint / spectator caches instead.
- **Spectator-pane reuse**: `ui::window_spectator_cache::SpectatorFrameCache` holds the last painted `FrameDisplay` per non-focused `PaneId`, keyed by the same `PrewarmQuery` shape. `window_paint_spectators::build_spectator_pane_data` consults it before each spectator build and reuses the cached frame via `PrewarmQuery::is_compatible_for_motion` (caret-byte drift ignored, matching the focused pane's `last_painted_frame_display` contract). On cache miss the spectator build runs `FrameDisplay::build_viewport_measured` bounded by the spectator's own visible row range plus `VIEWPORT_OVERSCAN_ROWS`, keeping first paint to visible-row materialization; offscreen hit-test queries use row-index summaries and fallbacks until a complete index is available, while scrollbar bounds read the frame's display-row count. A large spectator whose only stale field is `wrap_width_dip` no longer paints old wrap geometry; it builds a current-wrap partial frame for the visible source range derived from the same-document seed frame and does not reseed the spectator cache with that partial. During live resize, a large stale spectator miss with a different stale field, such as `decoration_revision` after worker-stamp drift, uses the same paint-only partial path. Image-reservation-active spectators (γ) now participate in the cache via the `PrewarmQuery` reservation signature: a stable expanded-image set hits, a changed set misses on `image_reservations_signature` and rebuilds once (large reservation-bearing spectators still pay one full viewport cold build on a reservation change rather than a partial — the partial/spectator-worker fast paths remain reservation-gated for now). This replaced the prior unconditional bypass that cold-walked the whole document every paint for any pane showing an expanded image — the multi-pane cliff that reverted multi-line table cells (`features/tables.md` § Phase F). Spectator paint treats the body rect as viewport truth and clamps `scroll_y_dip` only when the visible range starts entirely beyond `FrameDisplay::display_line_count()` or the saved scroll is non-finite / negative. Near-EOF saved offsets that still reveal content are painted without mutating the local view, so focus changes do not shift the pane. Hit-tests routed through `Window::resolve_hit_test_frame_display` also consult `lookup_for_hit_test` so a click in a previously-spectator pane reuses its projection without cold-building. Trace label `paint:spectator_cache`; `miss=wrap_width_partial` names the bounded current-wrap fallback, `miss=live_resize_partial` names the large live-resize stale fallback, and per-paint totals appear in `paint:projection_stats` (`spectator_cache_hits` / `spectator_cache_misses`).
- **Spectator cache writers (3-way)**: `SpectatorFrameCache` is UI-thread-owned with three documented writers — (a) spectator paint on cache miss after building the frame, (b) focused-paint cache seeding (`window_paint/cache_seed.rs`), and (c) the UI-thread projection-worker drain. Drain runs in `window_paint.rs` after geometry is prepared and before spectator payloads are built; it consumes only non-focused live-pane results via `Window::drain_spectator_projection_worker_results`, drops orphan results for destroyed panes via `retain_results_for_live_panes`, and drops stale-stamp results by recomputing the live stamp and calling `diff_field`. Trace token: `event:spectator_cache_populate source=worker_drain|paint_epilogue pane_id=… document_id=… rope_rev=… decoration_rev=…|-1 elapsed_us=…`.
- **Pending-spectator partial**: when `SpectatorCacheLookup::Empty` or `Stale(_)` resolves AND an exact-stamp worker request is already pending for that pane (`Window::has_pending_spectator_projection`), spectator paint builds a current-geometry partial frame for the visible source range instead of cold-walking the full row index inline. The partial is paint-only — it is **not** inserted into `SpectatorFrameCache`; only the real worker result refills the cache. This keeps split-pane bodies non-blank while the worker catches up and keeps their wrap width current during resize. Trace token: `paint:spectator_cache miss=placeholder_pending_partial pane=… hit=false source_lines=… viewport=START..END`. Pending-less cache misses still take the existing cold fallback, except large stale spectator misses handled by `miss=wrap_width_partial` or live-resize `miss=live_resize_partial`; empty pending-less misses still cold-build so a missing dispatch stays visible.
- **Layout-template prewarm for live panes**: layout shortcut, maximize restore, and keyboard pane resize go through `Window::try_dispatch_layout_projection_worker_for_live_panes` (`window_projection_spectator.rs`), submitting one worker request per live pane with `reason=layout_change`. The submission uses the existing early-dispatch path; affected spectator panes produce the same `event:projection_worker_queue_depth reason=layout_change` evidence as the focused pane.
- **Input direct paths**: caret placement, mouse hit-test, and scrollbar geometry consume row-index summaries directly rather than invoking the whole-document walker. Caret anchoring reuses a motion-compatible painted frame or cached row index; on stable-shape miss it refreshes only caret-affected source-line counts from the previous painted row index, and only then falls back to a source-line floor estimate. Mouse hit-test reuses painted / spectator / mouse-cache frames, then a cached row-index viewport frame, then a source-line-floor fallback. Scrollbar geometry and `ViewState` clamps use concrete display-row content height (`FrameDisplay::display_line_count() * line_height`) rather than EOF slack or partial-density estimates. Ctrl+End / Shift+Ctrl+End defer the exact-bottom snap to the paint path: the command sets `Window::pending_doc_end_scroll` after moving the caret, the generic `ensure_primary_caret_visible` post-hook lands the viewport approximately near the new caret so the cold build covers the bottom rows, then `on_paint` snaps `scroll_y_dip` to `frame_display.display_line_count() * LINE_HEIGHT_DIP - viewport_h` against the canonical paint-frame `FrameDisplay` and re-invalidates. EOF appends on wrapped-looking documents use the same snap, but a proven final source-line append first applies a one-display-row minimum reveal so a provisional row count cannot settle at the old bottom. Doing the snap with the painter's own projection eliminates the row-count divergence (image reservations, fold ranges, decoration revision drift) that previously made a command-thread cold rebuild land short on image-bearing or mid-edit buffers.
- **Lookups**:
  - `display_line_count_for_source(line)` → number of display rows the source line occupies (>1 only for soft-wrap or folds).
  - `first_display_line_index_for_source(line)` → starting display row for a source line.
  - `display_line_index_for_source_pos(line, byte_in_line)` → row containing a source position.
  - `source_byte_in_line_to_display_utf16(line, byte_in_line)` → D2D layout coordinate for caret / selection rendering.
  - `source_to_display(SourceByte)` / `display_to_source(DisplayByte)` per `DisplayLineSpec`.
- **Hit-test**: `DisplaySegment::hit()` returns the segment's `SegmentHit` payload — the renderer / mouse handler use this for checkbox toggles, link follows, and footnote definition/reference jumps without re-parsing the line.
- **Footnote projection**: body references render as a smaller superscript replacement when no caret intersects the source `[^label]` span. A caret inside the span reveals the raw source bytes and keeps the hit metadata on the visible range. Definition labels are styled, and the full definition body carries reverse-jump metadata to the first reference.

## Defensive snap (post-bugfix)
Stale decoration spans during undo replay can carry byte offsets that don't land on UTF-8 char boundaries in the live rope. `build_line_segments` snaps every action's `start` / `end` and every style-overlay range to the nearest `line_text` char boundary (`snap_to_line_char_boundary`) before composing segments. This keeps `display_bytes` slicing safe even when decorations are one frame behind.

A complementary defensive clamp in `style_runs` (`end_b.min(last_idx)`) protects against any future producer that bypasses the snap.

## MRU Prewarm Contract

- **Idle detector**: prewarm runs only when the window is not minimized, smooth-scroll is inactive, the window-state save debounce is not pending, persistence reports zero unflushed bytes, no input has arrived for 120 ms, and `GetUpdateRect` reports no pending repaint.
- **Bounded tick**: the timer fires every 50 ms and processes at most one stage per idle tick.
- **Cancellation**: any keyboard input cancels the active buffer's queued/cached prewarm. Direct UI mutation paths that bypass `SelectionEdit` call `Window::cancel_display_prewarm_for_buffer`; selection edits call `cancel_active_display_prewarm`.
- **Invalidation**: the cache key includes rope and decoration revisions. Processing a target drops stale rope-revision entries; accepting a decoration result drops undecorated and older-decorated entries for that buffer.
- **Decoration fallback**: a viewport-stage map is reusable while the decoration worker is still warming. When current decorations arrive, the decoration revision invalidates the fallback so the next paint can use the decorated projection.

## API surface
- Re-exports from `crates/display_map/src/lib.rs`: `DisplayMap`, `DisplayLineSpec`, `DisplaySegment`, `SegmentHit`, `SpanStyle`, the four ID newtypes.
- `crates/render/src/display_projection.rs::FrameDisplay` is the per-frame consumer. The render crate is the only client; `core` never sees a display byte.

## Configuration
- `editor.word_wrap` (bool) — toggles soft-wrap.
- `markdown.reveal_mode` (`"block"` | `"line"`) — determines whether marker spans show as `Hidden` (default) or `Revealed` (kept visible with marker style).
- Wrap width is the painted body-text-column width, computed by `continuity_render::resolve_body_text_width_dip` for the focused pane from the same inputs the renderer's `ContentMargins` consume (viewport width, font size, `line_numbers`, `minimap`, `search_minimap_active`, `show_outline_sidebar`, `outline_sidebar_width_dip`, `distraction_free` + cap). Non-focused pane bodies use `continuity_render::pane_body::spectator_body_text_width_dip`, which reserves only the spectator body gutter/padding. These helpers keep each pane's wrap column lined up with its rendered right edge. **Both** the focused wrap *target* and the spectator wrap *target* reserve the same small right safety gutter inside that edge — `WRAP_SAFETY_MARGIN_EM` (¼ of the zoomed font size, `pub(crate)` in `window_display_prewarm/projection_inputs.rs`) — applied in `display_projection_metrics` (focused) and `window_paint_spectators` / `window_projection_spectator::spectator_wrap_width_dip` (spectator). Focused and spectator panes **must** budget the identical wrap width or focusing a spectator pane visibly rewraps its text. Paint geometry is unchanged; wrap just fires a hair earlier.
- **Continuation rows never exceed the wrap column** — two independent guards, both regression-detected by `event:soft_wrap_overflow` (`technical/trace-guide.md`):
  1. **Exact cross-segment carry-over.** The break walker (`builder/soft_wrap.rs::grapheme_word_break_points_styled`, mirrored for row counts in `builder/row_counts.rs`) measures each continuation row's carry-over width *exactly across styled-segment boundaries* (inline code, bold, links) by tracking the running width captured at the last word break — `(running + w) − running_at_word_break` — never re-measuring a current-segment-only suffix. The old suffix re-measure under-counted cross-segment carry-overs and over-filled continuation rows on multi-segment lines.
  2. **Hanging-indent continuation budget.** Wrap *continuation* rows are painted shifted right by the line's hanging indent (leading tabs/spaces + any list marker — see `wrap::hanging_indent_dip` / `FrameDisplay::hanging_indent_advance_dip`), so they are budgeted at `wrap_width − hanging_indent`, floored at `1 − MAX_HANG_INDENT_FRACTION` (= 25 %) of the column via `wrap::continuation_wrap_budget_dip`. The first row of a line keeps the full wrap width; only continuation rows take the reduced budget. The budget is applied in **lockstep at all three wrap-decision sites** — `builder/soft_wrap.rs` (materialize), `builder/row_counts.rs` slow path (count), and `wrap_profile::row_count_from_profile` (cached reinterpretation, which now takes the continuation budget). The painter clamps its own indent offset at `MAX_HANG_INDENT_FRACTION` so offset + budgeted width can never cross the right edge. **Invariant:** any new wrap-decision code (new fast path, new cache) must apply the same per-row budget, and the hang-indent formula must stay identical between the builder (`measure(" ")` / `measure("\t")`) and the painter (column/tab advance scalars), or indented wrapped lines overflow again. A cut at trailing whitespace may overshoot the budget by that whitespace's width — invisible, since DirectWrite ink width excludes trailing whitespace. Regression tests: `crates/display_map/tests/hang_indent_budget.rs`, `wrap_profile_round_trip.rs`.
- Wrap measurement uses a DirectWrite-backed `WidthMeasure` in UI paint, prewarm, caret-anchor, vertical-motion, and mouse projection paths when the window's text format is available. That keeps proportional prose fonts (`editor.font_family_prose`) from wrapping early under the old fixed `font_size * 0.55` scalar. The scalar `FixedCharWidth` path remains the non-UI/test fallback.

## Display-row index (ε.1)

A `DisplayRowIndex` lives alongside the realized `DisplayLineSpec` vector inside every `DisplayMap`. One `u16` row count per source line, prefix-summed in a Fenwick tree, answers every offscreen source↔display query in O(log n):

- `display_row_count()` — total visible rows (scrollbar height, scroll clamp bounds).
- `first_display_row_of_source_line(SourceLine)` — start row for any source line (folded lines collapse onto the next visible row, matching the legacy `source_to_display_line` semantics).
- `display_row_count_for_source(SourceLine)` — row count for a source line, `0` if folded.
- `source_line_for_display_row(u32)` — inverse lookup; transparently skips folded source lines.
- `source_lines_for_display_rows(Range<u32>)` (ε.2) — maps a display-row range to the source-line range that contributes to it. Drives viewport realization.

The builder fills the index inline as it pushes specs (so the round-trip cost is negligible) and stamps it with the source rope revision, decoration revision, soft-wrap width, an opaque font-state hash, and a stable fold-set signature. Dirty and splice rebuilds mutate row counts in-place under rope / decoration deltas.

`PartialRowIndexState` marks indices whose offscreen rows are placeholders. Exact answers are guaranteed inside `walked_source_range`; scrollbar geometry and scroll clamps read the concrete `display_row_count()` / `FrameDisplay::display_line_count()` carried by the current frame. That count can be conservative until the worker or a full fill lands, but it never creates scrollable blank space past EOF. `estimated_total_rows()` remains a partial-walker diagnostic and refinement signal, not the scrollable extent. Splice / targeted refresh helpers refuse partial inputs unless the caller is using the explicit partial dirty/splice path, so placeholder counts cannot be promoted as full truth.

Cross-cutting principle for ε/P18: the index is allowed to be incomplete, stale, or partially realized — but never wrong. `crates/display_map/tests/row_index_parity.rs` pins per-source-line and prefix-sum agreement with the realized spec vector across plain text, folds, soft-wrap, and random inputs; partial-walker proptests pin equality for the walked range.

## Viewport realization (ε.2)

`DisplayMapBuilder::build_viewport(visible_rows, overscan, measure)` materializes `DisplayLineSpec`s only for source lines whose display rows intersect `visible_rows` (expanded by `overscan` rows above and below — default 20, matching CodeMirror 6's "buffer above and below" pattern). Full-index callers compute the whole-document `DisplayRowIndex` in `crates/display_map/src/builder/row_counts.rs`; P18 partial callers compute only the viewport-priority row range first and carry an estimated total until the full fill lands.

`DisplayMap` carries `realized_row_start: u32` and `realized_row_range() -> Range<u32>`. Looking up a display line outside the realized window returns `None`; `range_intersect_display` emits absolute display-row indices via the offset. Off-viewport source lines must not be treated as folded: UI caret lookup first checks the whole-document row index, returns the source line's first row when row count is non-zero but specs are unrealized, and only walks upward when the row count is actually zero. `from_parts_viewport(...)` constructs a viewport build directly; `DisplayMap::new` / `from_parts` continue to produce full-document realizations (the legacy `DisplayMapBuilder::build` path).

UI integration: `crates/ui/src/window_paint.rs::on_paint` computes the visible display-row range from `Window.view.scroll_y_dip` and `viewport_height_dip` against `LINE_HEIGHT_DIP`, then the classifier decides whether to reuse, rebuild dirty/splice, realize the viewport, or run `ColdPartial` / `DirtyPartial` / `SplicePartial`. Cache hits whose realized range does not cover the current viewport fall through to a fresh viewport or partial build. Caret anchor/visibility, mouse hit-test, prewarm cache, and vertical motion use the same viewport builder when a compatible row index exists; caret placement has a targeted stable-shape row-index refresh and mouse hit-test has a source-line-floor fallback on row-index miss, so input paths do not run the whole-document walker. The full-document `Window::build_frame_display_with_options` entry point stays for callers that truly need every spec materialized.

## Wrap-width partial-eligible classifier

Large wrap-width drift (e.g. opening / closing the outline sidebar or
minimap on a buffer above the partial threshold) is routed through
`ColdPartial` rather than full `Cold`, so the row-count walker only
walks the viewport range first. The classifier asserts this via
`large_wrap_width_change_routes_to_cold_partial`. Sidebar toggles
emit `event:projection_worker_early_dispatch reason=toggle_outline|toggle_minimap`
to give the worker a head start before the next paint.

## Walker statistics

`continuity_display_map::WalkerStats` is a per-walk accumulator the row-count walker (`builder/row_counts.rs`) populates when the caller passes `Some(&mut WalkerStats)`. Zero overhead when `None`: each counter increment is gated behind a single `Option::is_some` check. Fields name the **decision path** each source line took, not raw runtime — combine with the outer span's `dur_us` to compute per-line cost:

- `lines_total` — total source lines walked.
- `lines_folded` — fully folded; contributed 0 rows; no segment build.
- `lines_unwrapped` — `wrap.enabled() == false`; trivial 1-row-per-line; no `WidthMeasure::measure` call.
- `lines_fastpath_upper_bound` — wrap enabled; fit via `max_byte_advance × byte_count` upper bound; no DirectWrite call.
- `lines_fastpath_segment_sum` — wrap enabled; fit via summed segment widths; one `WidthMeasure::measure` call per non-empty segment.
- `lines_slowpath` — wrap enabled; row count could not be proven by the trivial fit paths. This includes exact/profile wrap-cache hits as well as real grapheme-cluster break walks; use `wrap_cache_hits`, `wrap_profile_hits`, and `wrap_cache_misses` to split cached service from real slow walks.
- `measure_calls` — total `WidthMeasure::measure` invocations across all lines.

Surfaced API:

- `DisplayMapBuilder::build_viewport_with_stats(visible_rows, overscan, measure, stats)` — full viewport build with stats.
- `DisplayMapBuilder::compute_row_index_with_stats(measure, stats)` — walker only; returns the `Arc<DisplayRowIndex>` without materializing specs. Paired with `build_viewport_with_row_index` for split-span tracing on the UI thread cold-build path (see `paint-flow.md` § "Cold-build sub-spans").
- `FrameDisplay::compute_row_index_measured(...) -> (Arc<DisplayRowIndex>, WalkerStats)` — render-crate wrapper for callers that do not emit UI walker trace.
- `FrameDisplay::compute_row_index_measured_with_caches(..., WalkerCallReason)` — render-crate wrapper for split-span UI cold builds with shared run / wrap / segment caches. `WalkerCallReason` has exactly four trace spellings: `paint_cold`, `paint_dirty`, `viewport_realize`, and `prewarm`.
- `DisplayMapBuilder::refresh_row_index_source_lines(...)` / `FrameDisplay::refresh_row_index_source_lines_measured_with_caches(...)` — targeted stable-shape refresh used by caret placement when exact row-index cache lookup misses after an edit.

`paint:row_count_walker_stats` events emitted by UI paint paths carry these counters directly so a trace can distinguish "the walker is blocked on DirectWrite per-segment measurements" (high `lines_fastpath_segment_sum`) from "the walker is paying the slow grapheme-cluster break walk" (high `lines_slowpath`). The latter is the dominant cost shape on long-wrapped paragraphs in prose markdown and is the structural target for a future per-segment measurement cache.

The walker also emits shared-cache attribution: `run_cache_hits/misses`,
`wrap_cache_hits/misses`, and `segment_cache_hits/misses`. These counters
cover the row-count walker caches. Exact/profile wrap-cache lookup happens
immediately after the line projection stamp is computed, before segment
construction and fast-path measurement; hits can therefore report
`measure_calls=0`, `measure_us=0`, and `segment_build_us=0` for cached rows.
`SegmentCache` remains the non-ASCII shaping cache used only when no wrap-row
cache/profile hit exists. It exposes the same line-projection stamp helper to
the UI mouse segment-hit path so segment clicks can reuse cached projected
segments before resolving a fallback frame.

The line-projection stamp is line-local for ordinary caret reveal.
Pipe tables no longer contribute a caret-dependent bit — tables now
render as visual cells unconditionally (pipes always hidden), so
moving in or out of a table block doesn't flip the line's hide set
and doesn't invalidate row-count / segment-cache hits. See
`features/tables.md` for the always-rendered model.

Partial paint paths add `event:partial_row_index_walk`,
`event:partial_dirty_walk`, and `event:partial_splice_walk`; full-fill
installation emits `event:row_index_complete_fill` and
`event:scrollbar_geometry_refined`. These events describe the current
P18 contract: paint uses whatever frame is immediately available and
does not block waiting for the projection worker.

## Key files
- builder: `crates/display_map/src/builder.rs` + responsibility-scoped siblings under `builder/` (`segments.rs`, `segment_coalescing.rs`, `tests.rs`).
- line spec: `crates/display_map/src/line.rs`.
- segment: `crates/display_map/src/segment.rs`.
- style: `crates/display_map/src/style.rs`.
- ID newtypes: `crates/display_map/src/id.rs`.
- soft-wrap: `crates/display_map/src/wrap.rs`.
- row index (ε.1): `crates/display_map/src/row_index.rs` + `crates/display_map/src/row_index_fenwick.rs`.
- viewport realization (ε.2): `crates/display_map/src/builder.rs::build_viewport` + row-count walker `crates/display_map/src/builder/row_counts.rs`; partial walkers in `crates/display_map/src/builder/build_partial.rs`, `crates/display_map/src/builder/progressive_walker.rs`, and `crates/display_map/src/builder/progressive_walker/partial_variants.rs`; soft-wrap pass split out to `crates/display_map/src/builder/soft_wrap.rs`.
- dirty rebuild (ε.3): `crates/display_map/src/builder/rebuild_dirty.rs` (per-source-line spec reuse); `DisplayRowIndex::dirty_after_rope_edits` + `RowDirty` enum in `row_index.rs`; `Decorations::compute_incremental` schema in `crates/decorate/src/pool.rs::DecorateRequest` (ε.4 follow-up wires per-buffer `Tree` storage).
- dirty-set spill (post-ε.7): `crates/ui/src/window_projection_plan.rs::LARGE_DIRTY_SET_THRESHOLD = 1500`. `Window::realize_projection_build_kind`'s `Dirty` arm spills above-threshold rebuilds to the projection worker and paints `prev` (or a viewport-only cold build when prev doesn't cover the viewport) for the current paint. Origin: a 16.1 s `paint:frame_display:dirty_rebuild` on a 107 k-line buffer after a forced full tree-sitter re-parse produced a near-whole-document dirty set. The inline rebuild scales linearly with the dirty-set size (`row_count_for_source_line` per dirty line); 1500 caps the inline worst case at ~150 ms while still being well above the typical per-keystroke dirty count (1–N+1 lines for splice / dirty / decoration-diff paths). Trace event: `paint:frame_display:dirty_spilled`.
- decoration parse-revision invalidation (post-ε.5/ε.7): `ProjectionClassifyInputs::decoration_parse_advanced: bool` plus `Window::last_painted_decoration_parse_revision: Option<u64>`. `Decorations::transformed_through(deltas, new_revision)` re-labels stale-parse content with the *current rope rev* so the `IndexStamps.decoration_revision` of two paints can match even when the underlying parse content has changed (a fresh worker parse for the same rope rev arrives at the same label). Paint samples the worker's actual parse rev directly from `decoration_cache.get(id).revision` *before* the transform and stores it on `Window`; the next paint compares and sets `decoration_parse_advanced=true` when it differs. The classifier's covering-cache fast path rejects on the flag, falls through to the `decoration_advanced` branch, and emits a `Dirty` plan with `Decorations::diff_dirty_lines(transformed_prev, current, rope)` as the dirty set. Closes the "new markdown line stays raw until click-back-in" bug class.
- frame projection: `crates/render/src/display_projection.rs`.
- document-end scroll: `Window::pending_doc_end_scroll` (set by `editor.move_doc_end` / `editor.extend_doc_end` in `crates/ui/src/window_commanding/context.rs`), applied against the canonical paint frame in `crates/ui/src/window_paint.rs::on_paint` after `resolve_paint_frame_display`.
- scrollbar bounds: `crates/render/src/scrollbar.rs`; UI clamps in `crates/ui/src/window_runtime.rs`, `crates/ui/src/window_view.rs`, and `crates/ui/src/window_scroll.rs`.
- MRU prewarm cache: `crates/ui/src/display_prewarm_cache.rs`.
- MRU prewarm window integration: `crates/ui/src/window_display_prewarm.rs`.

## Relates to
- [Decoration](decoration.md) — supplies the inline + block spans that drive `Hide` / `Replace` actions.
- [Rendering](rendering.md) — every `IDWriteTextLayout` is built from `DisplayLineSpec::display_text()` + `style_runs()`.
- [Selections + edits](selection-edits.md) — caret coordinates stay in source bytes; display projection converts to D2D coords only for paint and hit-test.
- [Buffer](buffer.md) — source bytes remain canonical; the display map carries no buffer state.
