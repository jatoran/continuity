# decorate

Tree-sitter-driven incremental parsing and markdown decoration. Produces
hide/restyle/widget spans for the live-preview render pipeline.

Layer: foundation+1. Pure: depends on `tree-sitter` + `tree-sitter-md`
and is fed an `Arc<Rope>` by the caller. The worker materializes the
surviving request to a `String` after latest-wins coalescing, then runs
tree-sitter. Runs on the decoration worker pool.

`Decorations` carries (per revision): block spans, inline spans + marker
kinds, heading entries, syntax highlights, autolink ranges, rainbow
bracket depths, **Phase F3 `inline_color_spans`** (`==text==` highlight
markup and `{#rrggbb:text}` hex-colored text), and **Phase F4
`evaluated_tables`** (per-cell formula recompute results overriding the
cell's display string at paint time). Footnote support adds
`InlineKind::FootnoteReference` from text blocks plus
`InlineKind::FootnoteDefinition` from the whole-document
`footnote_definition_spans` pass so UI hover-peek and jump navigation can
resolve labels without reparsing.

ε.4 incremental parse is active in the worker pool. Each worker owns a
bounded per-buffer `BufferTreeCache` keyed by `(buffer, revision, source_len)`;
when a request carries covered point-augmented deltas from the last accepted
decoration revision, the worker applies all tree-sitter `InputEdit`s to the
cached tree and parses once against that edited tree. Full parse remains the
fallback for cold buffers, uncovered delta history, and source-length sanity
failures.

`DecorationCache` stores accepted decorations as `Arc<Decorations>`.
Hot paths that need an owned handle use `get_arc(...).cloned()` so warm
hits are refcount bumps, not deep clones of span vectors. UI trace
events include `event:decoration_parse_incremental buffer=… ranges=…
elapsed_us=… cached_source_len=…` and `event:decoration_parse_full
buffer=… reason=… elapsed_us=…`.
