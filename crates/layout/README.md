# layout

DirectWrite `IDWriteTextLayout` cache (keyed by buffer/line/revision/
font+DPI/wrap-width, LRU-bounded), soft-wrap geometry, and the bidirectional
source-byte ↔ glyph-position map used for hit testing and cursor motion.

Layer: middle. Depends on `win`.

`LayoutCache` exposes monotonic `LayoutCacheCounters` for manual paint
tracing. The UI snapshots these counters around renderer submission when
`CONTINUITY_UI_TRACE` is enabled; normal builds only pay the counter
increments already adjacent to cache lookup/insert.

`run_cache` is the row-count walker's shared measurement cache. It stores
fragment width advances keyed by `(content_stamp, FontStateId, locale,
fragment, style)` behind sharded `RwLock`s so the projection worker and
inline cold fallback can reuse DirectWrite measurement output without a
single global lock.

`view_state` holds the per-pane `ViewState` (scroll, zoom, soft wrap). Its
clamp admits a scroll-past-end overscroll allowance re-capped to
`viewport_h - line_height`; `compute_max_scroll_dip` / `ViewState::max_scroll_dip`
expose that ceiling so UI scroll corrections cap at exactly what the wheel
and `jump_to` admit.
