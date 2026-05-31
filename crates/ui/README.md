# ui

The only crate that touches HWNDs. Owns top-level windows, the pane
tree, tab strip, find bar, command palette, and the per-window message
pump. Each window runs on its own UI thread.

Layer: top. Depends on `render`, `layout`, `input`, `command`, `win`,
`theme`, `core`.

Per-feature handlers live in topic-named sibling files (no `mod.rs`).
F2's right-docked outline sidebar splits across `window_paint.rs`
(per-frame `OutlineData` build + body-width subtraction + layout cache)
and `window_outline.rs` (click hit-test on the cached layout +
`markdown.insert_toc` / `markdown.refresh_toc` mutation handlers).

F5's image-paste pipeline is split: `image_store.rs` is the pure
hash-dedup writer (`FNV-1a 64` → `images/<hash>.<ext>`, idempotent
on duplicate bytes), and `window_file_image_drop.rs` carries the
drag-drop branch (`Window::import_dropped_images`,
`is_dropped_image_path`). The clipboard CF_DIB consumer and the
WIC + D2D render path are pending sub-tasks — see
`.docs/design/features/image-paste.md`.

F3 / F4 markdown extensions ship the data layer, rope-mutation handlers,
and renderer overlays for both source-line and soft-wrap paint paths.
`window_markdown_inline.rs` carries the four `ViewContext` impls
(`markdown_highlight_selection`, `markdown_color_selection`,
`markdown_clear_inline_color`, `markdown_insert_table`), each routed
through `EditorHandle::apply_edit` as one undo group.
`window_hex_palette.rs` defines `HexInputMode` — the `PaletteMode`
that accepts only `[0-9a-fA-F]` and gates commit on the 3 / 4 / 6 / 8-
digit contract; the overlay-side wiring lands with the renderer
sub-wire.

Overlay text inputs share `text_input.rs` state. `window_overlay_input.rs`
owns the visible-overlay vs focused-input split; layout helpers pass caret
and selection ranges through to `render::overlay`.
The command palette keeps its capped result-window state in `palette.rs`;
`overlay_render_palette.rs` maps that window to at most 10 visible rows and
an overlay scrollbar, while `window_overlay_input.rs` handles row hover,
click selection, and wheel scrolling on the UI thread.
Find-bar-specific chrome is split by responsibility: `find_regex_help.rs`
owns control ids and regex snippet data, `overlay_render_find.rs` owns
button / tooltip / snippet layout and hit-testing, `find_scope.rs` filters
matches to selected byte ranges, `find_replace_plan.rs` builds replace
ops, and `window_find_replace.rs` applies replace-one / replace-all through
the core thread. The window remains the only owner of overlay hover state and
HWND cursor shape; core remains the only writer of buffer text.

The left file-tree pane is UI-thread state in `file_tree.rs` and
`window_file_tree.rs`. Directory enumeration stays on the file-I/O
worker (`file_io_directory.rs` + `file_io_worker.rs`); the UI stores only
bounded shallow listings and projects visible rows into `render::FileTreeDraw`.
The pane never exposes delete/rename/write directory operations.

Motion is UI-thread state on `Window`. `motion.rs` owns the shared
ease-out-cubic contract, reduced-motion policy, and `StaggerScheduler`;
`surface_motion.rs`, `chrome_motion.rs`, and `status_motion.rs` project
overlay/banner/chord, pane/tab chrome, and status-chip transient frames
into immutable render params. The render crate paints the supplied frame
but does not own timers or mutable animation state.

Footnote hover-peek is passive UI-thread state in `mouse::MouseState`, not a
modal overlay session. `window_footnote_hover.rs` resolves hovered footnote
references through the current decoration snapshot, arms the 300 ms dwell
timer, paints the definition body via `OverlayDraw`, and clears on mouse-out
or chord. Ctrl+click footnote navigation rides `SegmentHit` in
`window_mouse_segment_hit.rs` and mutates caret selection only through
`EditorHandle::set_selections`.

`projection_worker.rs` owns the ε.5 off-UI-thread viewport projection
worker foundation. `window_projection_worker.rs` (ε.5b) owns the
UI-thread integration: lazy spawn (`Window::ensure_projection_worker`),
per-paint stamp computation (`current_projection_stamp`), worker-result
acceptance (`try_use_worker_result` returns `Err(WorkerMissReason::…)`
with the reason named — `StampMismatch` carries a `StampMismatchField`
so the trace identifies *which* stamp field drifted), and request
construction (`build_projection_request`). `window_projection_plan.rs` (ε.5c) owns
the projection-plan classifier shared by both paths:
`classify_projection_build(ProjectionClassifyInputs<'_>) -> ProjectionBuildKind`
(`CacheHit | SelectionRebuild | Dirty | Splice | Cold | ColdPartial |
DirtyPartial | SplicePartial`) is computed once per paint. The inline realization runs through
`Window::realize_projection_build_kind`; the worker submission goes
through `ProjectionBuildKind::to_worker_plan() -> Option<ProjectionPlan>`.
One source of truth keeps a worker result from ever disagreeing with
what the inline path would have built; `CacheHit` maps to `None`
and skips worker dispatch outright. One `ProjectionWorker` per window,
owned by `Window` and joined on drop. `Window::on_paint` polls the
worker between the classification step and the rebuild branch; a
stamp-matched result skips the inline path entirely. Worker miss falls
through immediately to the inline realization of the same kind — the
worker only ever *upgrades* a paint when the result is already ready,
never downgrades or blocks it. Thread ownership: one worker thread per
worker; the UI thread is the sole producer (`submit`) and sole consumer
(`take_latest_result`). The request channel is latest-wins (the worker
drains queued requests every iteration and keeps only the most recent);
the result mailbox stores the latest published `ProjectionResult`.
`ProjectionStamp` hashes every input that changes pixels
(rope/decoration revision, caret/fold/image-reservation signatures,
wrap/font/viewport/overscan) so the UI thread can validate worker
results in O(1) per field and drop stale ones. `ProjectionRequest` /
`ProjectionResult` carry `target_pane: PaneId`; the worker queue
holds bounded results per target (was a single latest-result cell)
so focused paint and the spectator drain can consume their own
results independently. Request coalescing is latest-wins **per
target pane** — layout-template prewarm can keep one focused request
plus one request per spectator without letting a typing burst fan
out unbounded work. The exception is same-stamp partial-fill work:
an older `paint_partial_fill` request may be retained beside a newer
same-pane non-fill request when document / rope revision / font state /
wrap width still match, so a full row index is not starved by epilogue
churn. Newer partial fills still replace older partial fills, and any
stamp drift drops the older fill. Worker-result acceptance drains only the
requested target; destroyed-pane results are dropped via
`retain_results_for_live_panes`. DirectWrite measurement
runs on the worker thread via `MeasureMode::DirectWrite`, wrapping
shared `IDWriteFactory` and immutable `IDWriteTextFormat` handles in a
`SendCom<T>` newtype documented thread-safe per the DirectWrite
multi-threading spec.

`window_projection_early_dispatch.rs` (ε.5e) hooks the edit-completion
paths (`selection_dispatch::dispatch_selection_edit`,
`selection::{insert_text, delete_back, delete_forward}_at_selections`)
to call `Window::try_dispatch_projection_worker_early(reason, submit_reason)` as soon
as `editor.apply_selection_edit` returns, before the next `WM_PAINT`.
The helper rebuilds the same `PaintProjectionInputs` paint would build
and runs them through the *same* `current_projection_stamp` +
`classify_projection_build`, so the worker can never be asked for a
frame paint wouldn't accept. The fine-grained `reason` stays on
`event:projection_worker_early_dispatch`; `submit_reason` is the
coarse queue-depth category (`early_dispatch`, `layout_change`, or
`focus_change`). Focus changes have a focused prewarm policy in
`window_projection_early_dispatch/focus_prewarm.rs`: if the classified
frame is partial, or the current cache hit carries a partial row index, the
worker gets a `ProjectionPlan::Cold` immediately so the UI can paint the
bounded partial now and receive a full row index later. `cached_frame` is passed as `None` —
`prewarmed_frame_display` would need `.take()` (state mutation paint
owns); the classifier picks Splice / Dirty / Cold against
`last_painted_frame`. `Window::last_early_dispatch_stamp` coalesces
identical back-to-back submissions; image reservations ride through
the worker request and are validated by the projection stamp. The
post-paint dispatch in `on_paint` remains as the warm-up / fallback
and uses `paint_epilogue` or `paint_partial_fill`; a cache-hit partial
frame maps to a background `ProjectionPlan::Cold` rather than skipping
worker dispatch. Trace event
`event:projection_worker_early_dispatch reason=… submitted=… plan=…
stamp_rev=… skip=…` names the outcome.

`window_paint_trace.rs` owns manual performance trace aggregation for
`CONTINUITY_UI_TRACE`. `Window::on_paint` uses it to emit
`paint:projection_stats` (projection source, viewport/realized counts,
worker miss reason, image/spectator inputs) and `paint:render_stats`
(layout-cache deltas, rows drawn, feature counts). Selection-only
funnels emit `selection_set` / `selection_update`, drag selection emits
`mouse_drag_selection`, resize emits `resize_projection_inputs`
(including logical client size, physical renderer target, and renderer
resize status), `live_resize_renderer_resize_deferred` when a live
shrink-axis target rebind is delayed until paint,
`live_resize_renderer_resize_apply` when that delayed rebind is applied
right before renderer draw, `live_resize_update_window`
when live `WM_SIZE` flushes the pending repaint synchronously, and
`live_resize_dwm_flush` when shrink-axis ticks fence compositor
consumption. The main top-level HWND is created with
`WS_EX_NOREDIRECTIONBITMAP` because D2D/DXGI owns full-client pixels;
`WM_ERASEBKGND` is also handled to keep default background erasure out
of the live-resize path. `Window::invalidate` emits `invalidate_request`.
All of these remain UI-thread diagnostics; core buffer state is still
only mutated by `core`.
`window_paint/epilogue.rs` owns the paint-tail Win32 validation,
motion-timer arm, initial caret-blink arm, font-swap spectator nudge,
and post-`EndPaint` document-end repaint scheduling.

`window_trace_state.rs` adds compact state snapshots to the same trace.
Every WndProc timing row carries the active buffer/pane/tab context, each
focused paint emits `paint:window_state` with snapshot/caret/view/cache
summary fields, and the paint path emits `paint:no_snapshot` before a
clear-only frame when the focused `BufferId` has no core snapshot. Trace files
start with `trace_columns` and `trace_open` metadata rows (schema, sink,
flush mode, process/build/target, cwd, argv), so a standalone TSV contains
enough session context for later diagnosis.

`display_prewarm_cache.rs` owns the Phase β idle prewarm queue/cache for
MRU-adjacent buffers; `window_display_prewarm.rs` wires it into the
window timer and paint path. It is UI-thread state only: the queue stores
derived `FrameDisplay` projections keyed by buffer, rope/decorations
revision, folds, caret bytes, wrap width, and font state; `core` remains
the sole writer of buffer text.
`window_display_prewarm.rs` also centralizes frame-display construction so
paint, prewarm, caret anchoring, vertical motion, and mouse projection all use
DirectWrite glyph widths for soft-wrap when the text format is live, with the
fixed-width scalar kept only as a startup/test fallback. Whole-document
row-count walker calls from this construction path are labeled with
`WalkerCallReason`; caret placement, mouse hit-test, and scrollbar geometry use
direct row-index / display-row paths instead of binding walker reasons. The
caret miss path can patch a previous same-shape row index by refreshing only
caret-affected source lines.

`window_view.rs` owns the post-edit caret visibility guard. Unlike
`window_caret_anchor.rs`'s exact reflow anchor, `ensure_primary_caret_visible`
first reuses geometry-compatible `FrameDisplay` row indexes from the painted
frame, the mouse hit-test cache, or the spectator cache. That keeps typing in
a large wrapped buffer from synchronously rebuilding the whole row-count index
just to prove the caret is already visible; trace token
`ensure_primary_caret_visible_fast` records the cache source and scroll action.
The pure row-estimation helpers and focused tests live in
`window_view/caret_visibility.rs`; `window_view.rs` keeps the command-facing
scroll / zoom / reveal methods.

Ctrl+End / Shift+Ctrl+End defer the exact bottom snap to the paint path:
`editor.move_doc_end` / `editor.extend_doc_end` set
`Window::pending_doc_end_scroll` after moving the caret, the generic
`ensure_primary_caret_visible` post-hook approximates the bottom so the
next paint's cold build covers the bottom rows, then `on_paint` snaps
`scroll_y_dip` to `frame_display.display_line_count() * LINE_HEIGHT_DIP
+ END_OF_BUFFER_BOTTOM_PADDING_DIP - viewport_h` against the canonical
paint-frame `FrameDisplay`. If that
snap moves the view, the in-progress paint draws with the pre-snap scroll
value and schedules `reason=doc_end_snap` after `EndPaint`, so Win32 cannot
validate away the corrected-bottom repaint and the renderer never draws a
top-realized frame at the bottom scroll offset. Using the painter's own
projection eliminates the
row-count divergence (image reservations, fold ranges, decoration
revision drift) that previously made a command-thread cold rebuild
land short on image-bearing or mid-edit buffers.

`window_pane_layout_ops.rs` owns the layout-changing operations
(`apply_layout_shortcut`, `toggle_maximize_focused_pane`,
`resize_focused_pane`) split off `window_panes.rs` to keep that file
under the conventions cap. `apply_layout_shortcut` deliberately bypasses
[`Window::with_caret_line_anchored`] and uses
[`Window::refresh_focused_viewport_unanchored`] because the layout
shortcut resets `view = ViewState::new()` (scroll_y_dip = 0): the
anchor's "preserve old screen y" semantic does not apply, and its
restore phase would walk the row-count index at the new wrap width.
The caret stays approximately
visible via a cheap source-line-based `scroll_y_dip`; the first paint
after refines it using the real display-row index.

`window_buffer_tab_repair.rs` enforces pane-tree integrity and the focused-pane
live-buffer invariant for restored and reshuffled pane trees. On the UI thread,
restore, layout shortcuts, focus switches, tab closes, and focused-tab adoption
normalize group/tab structure before any direct `groups[focused]` /
`tabs[active]` lookup. Structural repair removes orphan groups/tabs, prunes
stale MRU entries, replaces empty or unusable leaves with fresh empty buffer
tabs, and clears stale maximize targets. Missing core snapshots are replaced by
fresh buffers allocated through `EditorHandle::open_buffer`, and the tree is
saved again so stale ids do not keep reappearing from session JSON. Trace events
`pane_tree_structure_repair` and `pane_tree_buffer_repair` are gated by
`paint_trace::is_trace_enabled()`.

The same guard is centralized in
[`Window::refresh_focused_viewport`]: when `view.viewport_*_dip == 0`
(i.e. the view was just reset by `switch_focus` into a never-focused
pane, `open_new_tab`, `split`, `adopt_buffer_as_new_tab`, or
`reopen_closed_tab`), the anchor is skipped and the unanchored variant
runs. This covers the click → `try_pane_body_focus_switch` →
`switch_focus` path that previously paid the same row-index restore
when clicking onto a pane that had no saved scalar state after
`apply_layout_shortcut` cleared the `panes` map. Trace token
`refresh_focused_viewport source=unanchored_view_reset` names the skip.

`window_spectator_cache.rs` owns the per-pane spectator `FrameDisplay`
cache used by `window_paint_spectators::build_spectator_pane_data`.
Each non-focused pane's last painted projection is stored keyed by
`PaneId` and the same `PrewarmQuery` shape used by the focused pane's
`last_painted_frame_display`. On the next paint, the cache reuses the
prior frame via `PrewarmQuery::is_compatible_for_motion` — caret-byte
drift is ignored, so typing in a small focused pane against a large
non-focused buffer no longer cold-builds the spectator projection on
every frame. On cache miss the spectator build
runs `build_frame_display_viewport` instead of the full-document
entry point, bounded by the spectator's own visible row range plus
`VIEWPORT_OVERSCAN_ROWS` — first-paint after a split into a large
spectator drops from full-document spec materialization to visible-
row materialization. The spectator's body rect drives the viewport
height because per-pane `view.viewport_height_dip` is only refreshed
while the pane is focused. Spectator paint clamps saved scroll only when
the visible range is entirely past the cached frame's display-row count
or the saved scroll is non-finite / negative; near-EOF offsets that still
show content are painted without mutating the pane's view. Reservation-active spectators
use the cache under a `PrewarmQuery` that includes the image / table-row
reservation signature, so expanded images and multi-row tables reuse only
geometrically identical projections. The same cache
serves the mouse hit-test path via `lookup_for_hit_test` — a click
into a previously-spectator pane reuses its last projection instead
of cold-building. Trace label `paint:spectator_cache` records hit /
miss / stale-field per pane; `paint:projection_stats` carries the
per-paint `spectator_cache_hits` / `spectator_cache_misses` totals.

For large spectators with stale cache geometry,
`build_spectator_pane_data` does not reuse the old wrapped frame and
does not inline a full-document row-count walk. This includes
`wrap_width_dip`, rope / decoration revision drift, and
`image_reservations_signature` drift from expanded images or table-row
reservations. It asks
`window_paint_spectators/realtime_miss.rs` to derive a visible source
range from a same-document seed frame (or from the visible display-row
floor when no seed exists), then builds a current-wrap partial frame.
Stale-geometry partials are not inserted into `SpectatorFrameCache`;
the worker drain or a later full compatible paint owns the complete
cache fill. Same-geometry `realized_miss_partial` frames are inserted
because they only extend a compatible cache entry's realized row window.
Spectator worker requests compute the same per-pane image / table-row
reservations and table suppression as paint, so the worker-drained full
frame is inserted under the reservation-bearing query that paint will
look up on the next frame.
The trace tokens are `paint:spectator_cache miss=wrap_width_partial`,
`miss=live_resize_partial`, and `miss=stale_partial`.

When a stale or empty spectator lookup already has an exact-stamp worker
request pending, the paint path uses the same current-geometry partial
build instead of `FrameDisplay::placeholder_unrealized`, so split panes
never go blank while the worker catches up. The trace token is
`paint:spectator_cache miss=placeholder_pending_partial`.
Pending-less empty misses still cold-build so the partial path cannot
hide a missing dispatch. Very large no-wrap spectators still use the
`stub_no_wrap` path; that frame is geometrically current because soft
wrap is disabled.

The focused pane has a parallel mechanism in
`window_paint/frame_resolution.rs::cold_deferred_stub_frame`. When
[`classify_projection_build`] returns `Cold` *and* the only geometry
shift between the cached frame and the current paint is
`wrap_width_dip` (rope revision and decoration revision match), the
inline row-count walker is skipped and the cached frame is painted as
a stub. The post-paint projection-worker dispatch still submits the
`Cold` plan, so the worker produces the real new-wrap frame on a
background thread; the next paint accepts that result and seeds the
caches. While the stub is in flight,
`Window::seed_paint_caches_after_resolve` (in
`window_paint/cache_seed.rs`) intentionally does NOT install the stub
as `last_painted_frame_display` or in the spectator cache — its row
positions are at the old wrap and a later motion-compat lookup keyed
by the *new* wrap would otherwise return geometrically-invalid layout
data. Trace token `paint:frame_display:cold_deferred` records the
substitution along with `stub_wrap`/`target_wrap`/`rope_rev`/`reason`
fields. Empirically the candidate stub comes from
`spectator_promote` immediately after `apply_layout_shortcut`: the
just-focused pane was a spectator on the prior paint, so the user is
already looking at exactly this layout. The visible reflow when the
worker delivers replaces a synchronous full row-index rebuild with an
immediate paint followed by a background build.

`window_mouse_hit_test.rs::resolve_hit_test_frame_display` is the
single resolver behind every mouse → buffer-position mapping
(`client_to_buffer_position`, `segment_hit_at_client`,
`cursor_over_ctrl_click_target`). Resolution order: the focused
pane's `last_painted_frame_display` via
`PrewarmQuery::is_compatible_for_hit_test` (looser than motion-compat
— rope and decoration revision drift between paint and click is
ignored because the click maps to **what the user saw**), then the
spectator cache by current `focused` pane, then the focused-pane mouse
hit-test cache, then a row-index-cache-backed viewport build. When the
row-index cache also misses, the resolver builds a source-line-floor fallback
frame with a synthetic one-row-per-source-line index; it does not run the
whole-document walker from the mouse input handler.
The fallback cache lives in `window_mouse_hit_test_cache.rs`; it stores
the frame, `PrewarmQuery`, and decoration context so
`window_paint/mouse_candidate.rs` can promote the same frame into the
following paint as `CachedFrameSource::MouseHitTest`. That path removes
the click-then-paint duplicate row-index walk in large buffers. Trace
labels: `click_hit_test_frame_source` for hit-test source and
`mouse_hit_test_frame_display` for paint promotion / miss reason.
Segment-click handling additionally checks the shared P18 `SegmentCache` with
the row-count walker's line-projection stamp before resolving a fallback frame.

`window_caret_anchor.rs::resolve_caret_display_line` is now the δ.3
capture/restore consumer of the same reuse contract — it tries
`last_painted_frame_display` (motion-compatible only — font-scale or
wrap-width changes mid-anchor would corrupt the restore) and falls
back to a viewport-only build when a compatible cached `DisplayRowIndex` is
available. If the exact row-index cache misses but the previous painted frame
has the same source-line shape, it refreshes only the new caret line plus the
previous caret-reveal lines and materializes the caret line's display-row
range. If no previous same-shape index exists, it returns a source-line floor
estimate rather than invoking the walker on the anchor path. If the row index
says the caret's source line exists but the viewport-realized specs do not
include it, the resolver uses the row index instead of treating the line as
folded; wrapped multi-row lines get a second caret-line viewport realization
for exact continuation rows when a row index is available. Trace labels:
`caret_anchor_frame_source` and `caret_display_line_lookup`.

`window_decoration.rs` owns decoration-worker request production and result
drain on the UI thread. ε.4 requests use the UI-side `DecorationCache`
revision as the previous parse point, ask `core` for point-augmented rope
deltas, and emit `decoration_parse_incremental` / `decoration_parse_full`
trace events when worker results are accepted or rejected.

`window_tutorial.rs` opens the tutorial tab from `help.tutorial` and the
first-launch hook: constructs a `Buffer::synthetic_read_only` from
`continuity_command::TUTORIAL_MD`, adopts it through the core thread
(which skips persist for synthetic buffers), and installs it as a tab.
Idempotent — re-invocation refocuses the existing tab. Pinned by
`view_options.tutorial_buffer_id`. See `.docs/design/features/tutorial.md`.

`window_scroll.rs` owns wheel inertia state and the smooth-scroll
animation tick. Plain wheel input is resolved from the client cursor
point to the hovered pane body; pane chrome inside the body scrolls
that pane, while tab strips, status/title surfaces, overlays, and
active drags do not fall through. Focus is unchanged. Wheel impulses
accumulate velocity in DIPs per second for the target `PaneId`; the
existing `SCROLL_ANIM_TIMER_ID` advances that pane's
`ViewState.scroll_y_dip` fractionally with a 60 ms time constant until
velocity drops below 50 DIP/s. Reduced motion keeps the whole-line
instant jump. Inertia is cancelled by editor input, click, scrollbar
drag, splitter, buffer / pane switch, DPI change, and anchored reflow.
Scroll extents use concrete display-row content height from the current
`FrameDisplay` for wheel and scrollbar clamps. Keyboard/caret EOF reveals
add a single line-height bottom inset so the final display row is never
parked directly on the viewport clip edge.
The sliding-window scroll-prewarm dispatcher
(`maybe_submit_sliding_scroll_prewarm`) runs once per paint while
focused-pane inertia is active and the realized lead falls below half a
viewport in the direction of velocity. Per-paint scroll metrics emit on
`event:scroll_path`.

`window_paint/frame_resolution/scroll_anim_action.rs` is the pure
heuristic that picks among `Reuse | StripRealize | Placeholder` for a
scroll-anim paint that reuses a compatible cached frame. Strip realize
extends the previous frame's realized window when the uncovered row
gap fits within `SCROLL_ANIM_STRIP_REALIZE_MAX_ROWS = 80`; the
placeholder branch reuses the prev frame as-is and lets the
renderer's placeholder strip cover the gap.

`window_mouse_autoscroll.rs` owns the text-selection autoscroll timer
(16 ms) that fires while the user drags a selection past the focused
pane's body edge. Direction and last cursor live on `mouse::Autoscroll`;
the timer extends the selection at the clamped body edge and stops
on button-up / capture loss / re-entry into the body / scroll clamp at
the document edge.

`window_close_confirm.rs` owns the Ctrl+W dirty-tab confirmation arm.
First Ctrl+W on a dirty tab raises a transient banner; second Ctrl+W
within 3000 ms targeting the same `(PaneId, BufferId)` commits the
close. Cancel triggers: any other command, editor-body click,
focused-pane change, app focus loss, mouse wheel, normal tab
activation, clean close.

`window_theme_apply.rs` splits theme picker preview from commit.
`apply_theme_entry` is preview-only (no disk write, no broadcast);
`commit_theme_entry` writes settings.toml atomically and synchronously
refreshes the shared `LiveReload.initial` cell so newly-spawned
windows see the freshly-committed theme. The watcher echo
(`ConfigEvent::Settings`) drives the actual in-memory swap via
`apply_settings_theme_bindings`. Single source of truth: settings.toml.

`live_reload.rs` owns the `LiveReload` shared settings cell
(`Arc<Mutex<Settings>>`). The registry main thread is the sole writer
(via `replace_settings`); window threads only clone the snapshot via
`current_settings`. Cell-update precedes fanout, so any window spawn
sequenced after a commit observes the updated cell.

`window_tab_drag.rs` is the pure four-case tab-drop resolver
(`compute_tab_drop_resolution`) plus the cross-window
`Continuity.TabDragHover` broadcast send/receive. Preview
(`WM_MOUSEMOVE`) and commit (`WM_LBUTTONUP`) both call the same
helper so the painted affordance can never disagree with the commit.
`window_tab_drag_overlay.rs` translates the live `TabDrag` (and any
foreign-window hover broadcast) into the renderer-facing
`TabDragOverlayDraw`.
`window_tab_drag_ghost.rs` owns the source-side screen-space tab
replica shown while the drag is in flight. It is a no-activate popup
created, moved, and destroyed on the source window's UI thread; it uses
the active tab theme colors, strip height, tab-slot width calculation,
close glyph visibility, and active border color so the cursor carries
the same tab the user grabbed even when the cursor leaves the source
HWND. Mouse tear-off also passes the release screen point through
`window.tear_off_focused_tab`; the registry prefers that explicit
origin over the normal cascade point for that one spawned window.

`window_code_copy_hover.rs` owns the fenced + inline code-block copy
affordance. Hover detection re-runs per `WM_MOUSEMOVE`; click reads
the live rope via `fenced_inner_text` / `inline_code_inner_text` and
writes to the clipboard. The 1500 ms feedback timer
(`CODE_COPY_FEEDBACK_TIMER_ID`) flips the chip back to idle.
Caret-inside-block re-verification at paint time hides the button
even without a mouse move (e.g. Ctrl+End landing inside the hovered
block).

`window_projection_spectator.rs` owns the spectator-side worker
integration: `Window::try_dispatch_layout_projection_worker_for_live_panes`
fans one `reason=layout_change` worker request per live pane on layout
shortcut / maximize / keyboard resize;
`Window::drain_spectator_projection_worker_results` runs after paint
geometry is prepared and writes accepted results into
`SpectatorFrameCache` (with stale-stamp / destroyed-pane rejection);
`Window::has_pending_spectator_projection` is the predicate the
spectator paint path consults to decide between a bounded current-
geometry partial frame and the cold fallback on a stale or empty miss.
Spectator request stamps include the same image / table-row reservation
signature and per-pane table suppression as paint, preventing
reservation-bearing 2x2 grids from rebuilding `stale_partial` frames
every paint while the worker result is actually compatible.
`SpectatorFrameCache` keeps no internal locking; the three writers
(spectator paint, focused-paint cache seeding, worker drain) all run on
the UI thread.

`window_outline_entries_cache.rs` is the UI-thread
`(BufferId, rope_revision, decoration_revision)` cache shared by
outline paint and outline click hit-test. Empty-heading snapshots are
keyed with `decoration_revision = None` so a later decoration
delivery misses and rebuilds. Trace event: `event:outline_entries`.

`window_image_animation.rs` drives animated-GIF frame advancement.
`ensure_image_animation_timer` is called after every `WM_PAINT`; if
`Renderer::has_animated_images()` is true and the timer isn't already
armed, it starts a 50 ms `WM_TIMER` (`IMAGE_ANIMATION_TIMER_ID = 12`).
The tick calls `Renderer::advance_image_animations(now_ms)`, invalidates
the window if any frame changed, and auto-disarms when the cache drops
back to all-static. Armed state lives on `view_options.image_animation_timer_active`.
