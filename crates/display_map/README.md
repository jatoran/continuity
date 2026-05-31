# continuity-display-map

Builds an immutable `Arc<DisplayMap>` snapshot that translates between **source bytes**
(what the rope and persistence layer store) and **display bytes** (what the renderer
actually paints).

The crate sits between `decorate` (which produces `Decorations`) and `render` /
`layout` (which build `IDWriteTextLayout`s). The rendered layout never holds source
bytes that aren't supposed to be visible — markers (`**`, `## `), fence ticks, fold
ranges, and list-marker bullets are out-of-layout rather than painted over. Reveal,
soft-wrap, and folding are first-class inputs to one mechanism.

Place in the layer graph (`code_organization.md`):

```
display_map ← buffer · decorate
render      ← display_map · layout · …
ui          ← display_map · render · …
```

Single-writer rule: the `DisplayMap` is built on the decoration worker thread (or
another non-UI worker) and consumed by the UI thread as `Arc<DisplayMap>` — same
shape as `RopeSnapshot` and `Decorations`.

See spec.md §5 (rendering) and §9 (markdown live preview) for the invariant.

`table_hide_provider` is a sibling module called from `builder/segments.rs`. When
the caret is outside a pipe-table block, it emits `Hidden` ranges for every `|`
byte, every formula override's source payload, and the entire alignment-row line
so the visual-table painter in `continuity_render::table_paint` can draw cell
borders + aligned text without underlying source bytes bleeding through or raw
formula tails creating wrap continuations. Caret-inside-block tables fall through
with no hides so the user edits raw markdown. See `.docs/design/features/tables.md`.

Footnote references are another derived projection: off-caret `[^label]`
ranges become superscript label replacements with `SegmentHit::FootnoteReference`.
When the caret is inside the source range, the raw bytes are revealed and keep
the same hit metadata. Definition labels are styled, and the full definition
body carries `SegmentHit::FootnoteDefinition` for reverse Ctrl+click navigation.

`image_row_reservation_provider` is a γ-phase sibling that produces
per-source-line phantom-row counts for expanded inline images. The
builder consumes the slice via `DisplayMapBuilder::with_image_reservations`
and injects empty `DisplayLineSpec`s (zero source bytes, shared
`source_line` with the image's natural row) so subsequent text flows
beneath the bitmap rather than being overdrawn. Collapsed images and
expanded images whose native dimensions are not yet cached emit no
reservation. See `.docs/design/features/image-paste.md` § Row reservation.

`row_index` (ε.1) holds a `DisplayRowIndex` — one `u16` row count per
source line plus a Fenwick prefix-sum tree — that backs every
offscreen source↔display query (scrollbar height, EOF visibility,
caret anchoring, hit-test, fold/wrap/image-reservation math). The
builder fills the index inline as it pushes specs, then stamps it
with the rope + decoration revisions, soft-wrap width, and an opaque
fold signature. `row_index_fenwick` is its O(log n) prefix-sum +
inverse-lookup backing; `find_by_prefix` transparently skips folded
source lines. `source_lines_for_display_rows(Range<u32>)` powers ε.2's
viewport realization by mapping a display-row range back to the
source-line range that contributes to it.

`DisplayMapBuilder::build_viewport(visible_rows, overscan, measure)`
(ε.2) materializes `DisplayLineSpec`s only for source lines that
intersect the painted viewport (plus an overscan band — default 20
rows). Full-index callers still compute a whole-document
`DisplayRowIndex` through `builder/row_counts.rs`, so offscreen
consumers get exact answers when the index is complete. Large-buffer
paint can instead call the P18 partial walkers in
`builder/build_partial.rs` / `builder/progressive_walker.rs`: they
walk the viewport-priority source range plus safety margin, store
exact row counts inside that range, and mark offscreen rows as
placeholder-backed through `PartialRowIndexState` until the background
fill replaces them. `DisplayMap` tracks `realized_row_start`;
`display_line(idx)` returns `None` for rows outside the realized
window. Soft-wrap helpers live in `builder/soft_wrap.rs` (split out to
keep `builder.rs` under the 600-line conventions cap).

Whole-document row-count walker callers are gated by
`WalkerCallReason`: `PaintCold`, `PaintDirty`, `ViewportRealize`, and
`Prewarm`. Caret placement, mouse hit-test, and scrollbar geometry are
direct `DisplayRowIndex` / display-row consumers; they must reuse a
compatible index, refresh a bounded source-line set on top of a
same-shape previous index, or use a bounded fallback rather than
invoking the walker.

`wrap_cache` and `segment_cache` are the row-count walker's shared
slow-path caches. `wrap_cache` is geometry-dependent and remains keyed
by line projection content + font state + locale + `wrap_width_dip`.
`segment_cache` is the width-independent shaping layer: its
`SegmentCacheKey` is `(content_stamp, font_state)`, and each
`font_state` bucket owns a 16,384-entry sharded LRU by default.
Bucket count is bounded by `SEGMENT_CACHE_MAX_BUCKETS` /
`WRAP_CACHE_MAX_BUCKETS` (default 4); when a new bucket would exceed
the bound the least-recently-used bucket is dropped wholesale. The
walker checks exact/profile wrap rows immediately after computing the
line projection stamp, before segment construction or measurement, so
ASCII and non-ASCII cached rows can bypass both `segment_build_us` and
`measure_us`. `segment_cache` still stores only non-ASCII segment lists;
unchanged complex wrapped lines can reuse segment lists across
wrap-width changes when no wrap-row cache/profile hit exists.
`compute_line_projection_stamp` is the shared key helper for walker
and mouse segment-hit cache lookups.

Most caret input in that stamp is line-local, but pipe tables are block-
scoped reveal surfaces: any caret inside the table makes every table row
raw. For each table intersecting a line, the stamp records only that
boolean reveal state. Entering or leaving a table therefore misses stale
raw-vs-visual cached row counts, while moving within the same table keeps
cache reuse intact.

`DisplayMapBuilder::rebuild_dirty(prev, dirty, viewport, overscan,
measure)` (ε.3) clones `prev`'s row index, recomputes counts for the
dirty source-line range via the single-line cheap walker, derives the
new realized window from the updated index, and reuses `prev`'s
realized specs for every clean source line whose display rows already
sat inside `prev`'s realized window. Dirty source lines are
re-materialized through the shared `materialize_source_line` path so
output is byte-identical to a fresh `build_viewport`. Callers compute
the dirty range from `DisplayRowIndex::dirty_after_rope_edits(deltas,
rope_after)`. Three result shapes:

- `RowDirty::Lines(...)` — every delta sat inside an existing source
  line (within-line edit). Route through `rebuild_dirty`.
- `RowDirty::Splice(...)` (ε.3F / ε.3F+) — line-count change the
  index can splice in place. Covers single-`\n` Enter (`removed=1,
  inserted=2`), multi-line paste of `N` newlines (`removed=1,
  inserted=N+1`, 2026-05-17 extension), single-`\n` delete
  (`removed=2, inserted=1`), multi-line delete that removes `N`
  newlines (`removed=N+1, inserted=1`, 2026-05-17 extension —
  symmetric to multi-line paste; covers backspace-over-multi-line-
  selection, delete-to-end-of-paragraph, multi-line cut),
  single-delta replace-with-line-count-change, and multi-delta
  chains whose combined effect drifts the line count (2026-05-17
  ε.3F+ bracket extension — `bracket_splice` maps each delta's
  byte range forward through the chain, brackets the union into a
  single contiguous post-edit source-line range, and emits one
  splice that absorbs the whole region; the rapid-Enter / typing-
  burst case dominating the manual-lag trace). Route through
  `rebuild_spliced`.
- `RowDirty::FullRebuild` — genuinely ambiguous edits: nested
  deltas the bracket classifier can't resolve (a later delta
  overlaps a tracked byte), splices that would remove zero pre
  slots, or splices whose pre-slot range overruns the pre-edit
  source-line count. Route back to `build_viewport`.

`DisplayMapBuilder::refresh_row_index_source_lines(previous, lines,
measure)` is the smaller input-path helper for stable-shape misses. It
clones a previous whole-document index, recomputes only the named
source-line counts through the same single-line row-count path, advances
the stamps, and returns `None` when the source-line count changed.

The `rebuild_dirty` implementation lives in `builder/rebuild_dirty.rs`;
the `rebuild_spliced` implementation in `builder/rebuild_spliced.rs`
(generic over `splice.inserted` — the multi-line-paste extension
required no change to the splice builder, only to the classifier).
