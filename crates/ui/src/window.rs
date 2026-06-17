//! Top-level window: visible HWND, message pump, render dispatch, input
//! routing.
//!
//! **Thread ownership**: every `Window` is pinned to a single UI thread —
//! the one that registered its HWND and runs its message pump. Cross-
//! thread access goes through the registry's `WindowControl` channel.
//!
//! The `Window` type's surface is split across topical `window_*.rs`
//! siblings; this file carries the shared UI-thread state.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::buffer_history_tab::BufferHistoryTab;
use crate::display_prewarm_cache::DisplayMapPrewarm;
use crate::overlays::Overlays;
use crate::pane_state::PerPaneState;
use crate::pane_tree::{PaneId, PaneTree, TabId};
use crate::window_heading_lines_cache::HeadingLinesCacheEntry;
use crate::window_mouse_hit_test_cache::MouseHitTestFrameCacheEntry;
use crate::window_theme::ActiveTheme;
use continuity_buffer::BufferId;
use continuity_command::Registry;
use continuity_core::{EditorHandle, WpmTracker};
use continuity_decorate::{DecoratePool, DecorationCache, Language};
use continuity_display_map::{SegmentCache, WrapCache};
use continuity_keymap::{Conflict, Keymap};
use continuity_layout::{DWriteFactory, FontStateId, LayoutCache, RunCache, ViewState};
use continuity_persist::{MetricsDailyDelta, PersistClient};
use continuity_render::{FrameDisplay, Renderer};
use continuity_win::WindowClass;
use windows::Win32::Foundation::HWND;

pub(crate) use crate::window_constants::{
    END_OF_BUFFER_BOTTOM_PADDING_DIP, FONT_FAMILY, FONT_LOCALE, FONT_SIZE_DIP,
    LAYOUT_CACHE_CAPACITY,
};

pub(crate) use crate::window_timers::{
    CARET_BLINK_TIMER_ID, CONFIG_POLL_TIMER_ID, CONFIG_POLL_TIMER_MS, DECORATION_WATCHDOG_TIMER_ID,
    DECORATION_WATCHDOG_TIMER_MS, FILE_IO_TIMER_ID, FILE_IO_TIMER_MS, SCROLL_ANIM_TIMER_ID,
    SCROLL_ANIM_TIMER_MS, STATE_SAVE_DEBOUNCE_MS, STATE_SAVE_TIMER_ID, WHEEL_LINES_PER_NOTCH,
};

/// A visible top-level editor window bound to a single buffer.
pub struct Window {
    pub(crate) hwnd: HWND,
    pub(crate) _class: WindowClass,
    pub(crate) editor: Arc<EditorHandle>,
    pub(crate) buffer_id: BufferId,
    pub(crate) registry: Registry,
    pub(crate) keymap: Keymap,
    pub(crate) default_keymap_toml: &'static str,
    pub(crate) user_keymap_path: Option<PathBuf>,
    pub(crate) keymap_conflicts: Vec<Conflict>,
    /// Pending chord prefix — chords typed since the user started a
    /// multi-key sequence like `Ctrl+K, Ctrl+R`. Cleared on dispatch,
    /// on a non-matching follow-up, or on context change.
    pub(crate) pending_chord_sequence: Vec<continuity_input::KeyChord>,
    pub(crate) dwrite: DWriteFactory,
    pub(crate) renderer: Option<Renderer>,
    pub(crate) text_format: Option<windows::Win32::Graphics::DirectWrite::IDWriteTextFormat>,
    pub(crate) shift_held: bool,
    /// Client width in physical pixels. Layout code converts through
    /// `window_dpi` before producing DIPs.
    pub(crate) client_width: u32,
    /// Client height in physical pixels. Owned by this window's UI thread.
    pub(crate) client_height: u32,
    /// Pending live-resize renderer target in physical pixels. Owned by
    /// this window's UI thread; used to delay shrink rebinds until paint
    /// is ready to draw the replacement frame.
    pub(crate) deferred_renderer_resize: Option<(u32, u32)>,
    /// Current per-window DPI. Updated only on the owning UI thread from
    /// `GetDpiForWindow` during setup and from `WM_DPICHANGED`.
    pub(crate) window_dpi: u32,
    /// `true` while `WM_DPICHANGED` is applying Windows' suggested rect;
    /// any re-entrant `WM_SIZE` waits for the DPI handler's anchored reflow.
    pub(crate) is_applying_dpi_change: bool,
    /// Set to true once we've completed initial render setup; avoids racing
    /// the first WM_PAINT before we know the client size.
    pub(crate) inited: bool,
    /// Active overlay state machine (find/palette/quick-open/goto…).
    pub(crate) overlays: Overlays,
    /// `true` while an overlay text input owns keyboard focus. Owned by
    /// this window's UI thread; visibility remains in `overlays`.
    pub(crate) overlay_input_focused: bool,
    /// Last-clicked time + line for triple-click detection.
    pub(crate) mouse_state: crate::mouse::MouseState,
    /// Per-pane runtime view state (scroll, zoom, soft-wrap).
    pub(crate) view: ViewState,
    /// Bounded LRU cache of `IDWriteTextLayout` per visible logical line.
    pub(crate) cache: LayoutCache,
    /// Shared row-count run cache. The projection worker is the primary
    /// mutator; inline UI fallback uses the same sharded backing store.
    pub(crate) walker_run_cache: Arc<RunCache>,
    /// Shared row-count wrap cache. The projection worker is the primary
    /// mutator; inline UI fallback uses the same sharded backing store.
    pub(crate) walker_wrap_cache: Arc<WrapCache>,
    /// Shared row-count segment cache. The projection worker is the primary
    /// mutator; inline UI fallback uses the same sharded backing store.
    pub(crate) walker_segment_cache: Arc<SegmentCache>,
    /// Hash describing the active font (family, size at current zoom,
    /// locale). Used as a cache key component.
    pub(crate) font_state: FontStateId,
    /// `true` while the smooth-scroll WM_TIMER is running.
    pub(crate) scroll_anim_active: bool,
    /// P12 wheel-inertia state. UI-thread-owned; ticks on the existing
    /// smooth-scroll timer and writes only `view.scroll_y_dip`.
    pub(crate) scroll_inertia: crate::window_scroll::ScrollInertia,
    /// `true` while the shared motion WM_TIMER is running.
    pub(crate) motion_timer_active: bool,
    /// Phase-10 decoration worker pool. `None` until `install_decorate_pool` — deferred so test harnesses skipping worker spawn still operate.
    pub(crate) decorate_pool: Option<DecoratePool>,
    /// Per-buffer cache of the latest accepted decoration snapshot.
    pub(crate) decoration_cache: DecorationCache,
    /// Most recent revision submitted to the worker pool. Used to skip
    /// duplicate requests when the buffer hasn't advanced.
    pub(crate) last_submitted_decoration_revision: Option<u64>,
    /// Per-buffer decoration revisions already submitted to the worker
    /// pool. Spectator decoration submission runs from paint, so this
    /// UI-thread-owned map suppresses duplicate in-flight requests for
    /// non-focused buffers whose cache has not caught up yet.
    pub(crate) last_submitted_decoration_revision_per_buffer: RefCell<HashMap<BufferId, u64>>,
    /// Sorted snapshot of the buffer ids whose tree-sitter trees were kept
    /// alive at the last decoration-tree prune (Block 2.1). UI-thread-owned;
    /// compared each paint so `prune_offscreen_decoration_trees` only evicts
    /// when the visible / MRU keep-set actually changes.
    pub(crate) last_tree_prune_keep: RefCell<Vec<u128>>,
    /// Per-buffer cache of the most recent non-empty visual-table layouts
    /// for the focused pane. When `compute_table_layouts` returns an
    /// empty list — typically because the decorate worker lags the rope
    /// by a frame after a keystroke and `build_one_table_layout`'s
    /// byte-to-char guard rejects the stale block range — we fall back
    /// to the cached layout so the chrome paints continuously instead
    /// of dropping for a frame. Mouse hit-testing also reads from this
    /// cache so clicks resolve to cells even when the latest frame's
    /// layout is briefly empty.
    ///
    /// Stored as `Arc<Vec<TableLayout>>` so the per-frame cache insert
    /// is a refcount bump rather than a deep clone of the cell vector
    /// — `TableLayout` contains `String` cell text whose deep-clone
    /// would dominate the per-keystroke paint cost on large tables.
    /// Owned by this window's UI thread.
    pub(crate) last_focused_table_layouts:
        RefCell<HashMap<BufferId, Arc<Vec<continuity_render::TableLayout>>>>,
    /// `[workers].decoration_watchdog_ms`, mirrored on the UI thread and
    /// pushed into the decoration pool on hot reload.
    pub(crate) decoration_worker_watchdog_timeout_ms: u32,
    /// `true` while the decoration-watchdog event poll timer is running.
    pub(crate) decoration_watchdog_poll_active: bool,
    /// Cached language identifier for the active buffer.
    pub(crate) language: Language,
    /// Revision the cached language was computed against.
    pub(crate) language_revision: Option<u64>,
    /// Per-window theme state (mode, system_dark flag, current resolved
    /// theme). Owned by this window's UI thread.
    pub(crate) active_theme: ActiveTheme,
    /// Last `DWMWA_USE_IMMERSIVE_DARK_MODE` value pushed to the OS title
    /// bar. `None` until the first sync. Guards [`Self::sync_titlebar_theme`]
    /// so the DWM syscall only fires when the resolved theme's light/dark
    /// cast actually changes. Owned by this window's UI thread.
    pub(crate) titlebar_dark_applied: Option<bool>,
    /// Last window-caption text applied via `SetWindowTextW`. `None`
    /// until the first sync. Guards [`Self::sync_window_title`] so the
    /// syscall only fires when the active tab's label actually changes.
    /// Owned by this window's UI thread.
    pub(crate) window_title_applied: Option<String>,
    /// Phase-11 per-window view-options state (line numbers, minimap,
    /// ruler columns, caret style, …).
    pub(crate) view_options: crate::window_view_options::ViewOptions,
    /// Default right-edge chrome visibility for buffers without overrides.
    pub(crate) right_edge_chrome_defaults: crate::window_right_edge_chrome::RightEdgeChromeState,
    /// Per-buffer right-edge chrome overrides.
    pub(crate) right_edge_chrome_by_view:
        HashMap<BufferId, crate::window_right_edge_chrome::RightEdgeChromeState>,
    /// Currently active prose font family. Switched by `view.set_font_family`.
    pub(crate) prose_font_family: String,
    /// Per-pane font-size override in DIPs (set by `view.set_font_size`).
    /// `None` means use the default `FONT_SIZE_DIP`.
    pub(crate) font_size_dip_override: Option<f32>,
    /// Deferred font-change request, set by `view.set_font_family` /
    /// `view.set_font_size` / settings.toml reload on commit. While
    /// `Some`, projection stamps use `pending.target_font_state` so the
    /// worker rebuilds the display map for the new font in the
    /// background; the live `prose_font_family`/`font_size_dip_override`
    /// pair stays on the *previous* font so the body keeps painting
    /// without an overflow flash. When the worker delivers a result
    /// matching the pending font_state, [`Window::try_apply_pending_font_swap`]
    /// performs the atomic state swap inside `with_caret_line_anchored`
    /// and clears this field. See `crate::window_font_swap`.
    pub(crate) pending_font_change: Option<crate::window_font_swap::PendingFontChange>,
    /// Watchdog for the font-swap settle loop. Set to
    /// `Some(now + ~2s)` by [`Window::try_apply_pending_font_swap`]
    /// when a focused-pane swap fires; while `Some` and unexpired,
    /// the end of `on_paint` nudges another `invalidate_hwnd` if any
    /// spectator cache entry still carries a pre-swap `font_state`.
    /// Cleared when every spectator catches up *or* the deadline
    /// passes (whichever comes first), so the message pump never
    /// spin-paints once steady state is reached. See
    /// `crate::window_font_swap::nudge_font_swap_settle`.
    pub(crate) font_swap_settle_deadline: Option<std::time::Instant>,
    /// δ.6 Tier 2 — bundle of `[editor]` / `[markdown]` projections that
    /// were previously consumed off `Settings` per-call (no projection
    /// site existed). Lives on a sibling sub-state so the canonical
    /// `Window` struct stays under the 600-line cap. See
    /// [`crate::window_settings_projections::SettingsProjections`].
    pub(crate) settings_projections: crate::window_settings_projections::SettingsProjections,
    /// `true` while the caret blink phase is in its "visible" half.
    pub(crate) caret_blink_visible: bool,
    /// Tick of the last keystroke; the blinker stays "visible" while the
    /// user is actively typing (within the blink period).
    pub(crate) last_input_tick: u64,
    /// `true` while the caret-blink WM_TIMER is running.
    pub(crate) caret_blink_active: bool,
    /// Phase-12 live-reload metadata (initial settings + paths +
    /// persist-mode applier). The watcher receiver itself is no longer
    /// per-window — see `control_rx`.
    pub(crate) live_reload: Option<crate::live_reload::LiveReload>,
    /// Phase-16.5 registry → window control receiver (drained on config-poll tick).
    pub(crate) control_rx: Option<crate::WindowControlRx>,
    /// `true` while the config-poll WM_TIMER is running.
    pub(crate) config_poll_active: bool,
    /// Phase-14 persistence wiring (window_id, save / delete sinks,
    /// optional restored state). `None` for tests and the headless canary.
    pub(crate) persistence: Option<crate::window_placement_persistence::WindowPersistence>,
    /// Phase-15 file-I/O worker client and per-window open replies.
    pub(crate) file_io: Option<crate::file_io::FileIoClient>,
    pub(crate) file_open_tx: crossbeam_channel::Sender<crate::file_io::FileIoEvent>,
    pub(crate) file_open_rx: crossbeam_channel::Receiver<crate::file_io::FileIoEvent>,
    /// Registry-owned file-open router; callbacks cross through channels.
    pub(crate) open_file_window: Option<crate::window_config::OpenFileWindow>,
    /// Registry-owned file-buffer index updater.
    pub(crate) register_file_buffer: Option<crate::window_config::RegisterFileBuffer>,
    /// `true` while the file-I/O poll timer is running.
    pub(crate) file_io_poll_active: bool,
    /// Left file-tree pane state. Owned by this window's UI thread;
    /// directory reads are delegated to the file-I/O worker.
    pub(crate) file_tree: crate::file_tree::FileTreeState,
    /// `true` while the Phase-17 debounced state-save WM_TIMER is armed.
    pub(crate) state_save_pending: bool,
    /// Non-blocking banner for file-status prompts.
    pub(crate) file_banner: Option<crate::window_file::FileBanner>,
    /// One-shot dirty-tab close confirmation owned by this window's UI thread.
    pub(crate) unsaved_close_arm: Option<crate::window_close_confirm::UnsavedCloseArm>,
    /// One-shot status-bar notices owned by this window's UI thread.
    pub(crate) status_notices: Vec<crate::window_status_notice::StatusNotice>,
    /// Phase-14 IVirtualDesktopManager handle. Constructed lazily inside
    /// `Window::new` (already on the UI thread, COM apartment present).
    /// `None` when COM creation fails (downlevel host); the window keeps
    /// running, but desktop pin/restore become no-ops.
    pub(crate) virtual_desktop: Option<continuity_win::VirtualDesktopManager>,
    /// Phase-13 pane tree. The focused leaf's active tab determines
    /// `self.buffer_id`; the rest of the tree drives chrome/layout.
    pub(crate) tree: PaneTree,
    /// Per-pane state for *non-focused* leaves. The focused leaf's state
    /// is mirrored in the scalar `Window::view`/`buffer_id`/`language*`
    /// fields so existing single-buffer code paths keep working without
    /// per-callsite refactors. On focus switch, the scalars are swapped
    /// in/out of this map.
    pub(crate) panes: HashMap<PaneId, PerPaneState>,
    pub(crate) paste_history: crate::window_clipboard::PasteHistory,
    pub(crate) ime_state: crate::window_ime::ImeState,
    pub(crate) spell_state: crate::window_spell::SpellState,
    /// Phase-16.5 auto-pair config — mirrors `[editor].auto_pair_*`, refreshed via [`Self::apply_settings`].
    pub(crate) auto_pair: continuity_core::AutoPairConfig,
    /// Per-window indent config — mirrors `[editor].indent_type` /
    /// `indent_width` / `tab_width`, refreshed via
    /// [`Self::apply_indent_settings`] and mutated by the indent
    /// command family. Owned by this window's UI thread.
    pub(crate) indent: crate::window_indent::IndentConfig,
    pub(crate) intended_columns: Vec<u32>,
    /// Sticky display-byte offset within the head's wrapped display row.
    /// Parallel to [`Self::intended_columns`] — used by the soft-wrap
    /// vertical-motion branch so multi-step sticky behaviour works across
    /// rows of varying width (single-step came from the live head).
    pub(crate) intended_display_columns: Vec<u32>,
    pub(crate) intended_columns_for: Vec<continuity_text::Position>,
    pub(crate) jump_glow: Option<crate::jump_glow::JumpGlow>,
    /// α.1 edit-action echo (paste / duplicate / move-line / undo-target
    /// / smart-expand boundary). Single optional cell — the most recent
    /// pulse wins, matching the jump-glow contract.
    pub(crate) edit_pulse: Option<crate::edit_pulse::EditPulse>,
    pub(crate) caret_tween: Option<crate::caret_tween::CaretTween>,
    /// α.0 resolved motion policy. Owned by this window's UI thread.
    pub(crate) motion_policy: crate::motion::MotionPolicy,
    /// α.0 scheduler that offsets same-batch transitions by 60 ms.
    pub(crate) stagger_scheduler: crate::motion::StaggerScheduler,
    /// α.0 overlay/banner enter-dismiss motion.
    pub(crate) overlay_motion: crate::surface_motion::SurfaceMotionState,
    /// α.0 pane focus + tab activation motion.
    pub(crate) chrome_motion: crate::chrome_motion::ChromeMotionState,
    /// α.0 status-chip/value update transients.
    pub(crate) status_motion: crate::status_motion::StatusMotionState,
    /// α.0 hold-modifier chord HUD state.
    pub(crate) chord_hud: crate::chord_hud::HudState,
    /// α.0 chord HUD enter-dismiss motion.
    pub(crate) chord_hud_motion: crate::surface_motion::SurfaceMotionState,
    /// P0.8.3 transient "building view" overlay state. Armed by the
    /// bounded-wait helper when a layout/focus paint waits past the
    /// loading-overlay threshold; dismissed on the next worker-served
    /// frame.
    pub(crate) loading_overlay_state: crate::window_loading_overlay::LoadingOverlayState,
    pub(crate) trim_trailing_whitespace_on_save: bool,
    /// §H5 — mirrors `[editor].slash_commands_enabled`. Gates the
    /// typed-`/` line-start trigger; the explicit `Ctrl+/` chord
    /// dispatch also reads this so the feature is fully off when
    /// disabled.
    pub(crate) slash_commands_enabled: bool,
    /// §H5 — mirrors `[editor].slash_commands_palette`. `None` falls
    /// back to the registry's `palette_safe` ids; `Some(list)`
    /// restricts the palette to those ids in that order.
    pub(crate) slash_commands_palette: Option<Vec<String>>,
    /// F5: resolved shared image-store dir; `None` when `%APPDATA%`
    /// expansion fails (drag-drop image branch then falls through to
    /// tab-open).
    pub(crate) image_store_dir: Option<std::path::PathBuf>,
    /// F5: mirrors `[markdown].inline_images`; default on per
    /// spec-delta §L#3.
    pub(crate) inline_images_enabled: bool,
    /// F5 Pass 2: resolved `[ui].image_cache_bytes` value, applied to
    /// the renderer at construction time (before lazy renderer
    /// creation `set_image_cache_capacity` cannot reach the cache).
    pub(crate) image_cache_bytes_target: usize,
    /// F5 redesign — per-`(BufferId, URL)` expand state. Absent or
    /// `false` ⇒ collapsed (the default); `true` ⇒ expanded. The
    /// in-memory map is the live source of truth; `view_states`
    /// rows are hydrated into it on buffer open and flushed back
    /// on edit-debounce / window-close.
    pub(crate) image_expand_state: std::collections::HashMap<(BufferId, usize), bool>,
    pub(crate) find_memory: HashMap<BufferId, crate::find_bar::FindBarMemento>,
    pub(crate) find_persist_per_buffer: bool,
    /// Phase-I2 persist client for metrics recording / purge. `None`
    /// disables the metrics tap (test harnesses, headless canary).
    pub(crate) persist_client: Option<PersistClient>,
    /// Phase-I2 rolling-window WPM tracker (60 s default). Fed one
    /// timestamp per inserted character on the UI thread.
    pub(crate) wpm_tracker: WpmTracker,
    /// Phase-I2 in-flight metrics delta accumulated since the last 1 Hz
    /// flush to `record_metrics_delta`. Reset every successful flush.
    pub(crate) metrics_pending: MetricsDailyDelta,
    /// Phase-I2 wall-clock ms of the last metrics flush; the metrics
    /// flush only fires once per second.
    pub(crate) metrics_last_flush_ms: u64,
    /// Phase-I2 wall-clock ms of the most recent recorded keystroke.
    /// Used to compute `active_ms`, the rolling-WPM trailing point,
    /// and the §I2 idle-freeze threshold.
    pub(crate) metrics_last_keystroke_ms: u64,
    /// Phase-I2 last *live* WPM reading taken while the user was
    /// still inside the [`crate::window_metrics_paint::METRICS_WPM_IDLE_THRESHOLD_MS`]
    /// window. Painted in place of `wpm_tracker.wpm_now()` while idle
    /// so the value stops decaying once the user stops typing.
    pub(crate) wpm_frozen: u32,
    /// Phase-I2 `true` while the 1 Hz metrics repaint timer is running
    /// (set by [`Window::start_metrics_repaint_timer`]).
    pub(crate) metrics_repaint_active: bool,
    /// Phase-I1 cached time-machine preview rope. Populated lazily on
    /// the first `on_paint` after `view_options.overlay.pinned_revision`
    /// changes. The cached `Revision` lets `on_paint` skip the persist
    /// reload when the user holds the slider thumb steady.
    pub(crate) time_machine_preview: Option<crate::window_time_machine_ops::TimeMachinePreview>,
    /// Phase-I1 mouse-drag state for the time-machine slider. `Some`
    /// while the user holds the left mouse button down on the strip;
    /// `None` otherwise. Used by `window_mouse` to drive
    /// `compute_revision_for_x` on every `WM_MOUSEMOVE` and to release
    /// mouse capture on `WM_LBUTTONUP`.
    pub(crate) time_machine_drag: Option<crate::window_time_machine_ops::TimeMachineDrag>,
    /// δ.2 — per-command recency map for the command palette. Survives
    /// individual palette dismisses so muscle memory persists across
    /// open/close cycles within a single window session. Not
    /// persisted; resets when the window closes.
    pub(crate) palette_command_recency: HashMap<String, u64>,
    /// δ.2 — monotonic counter that backs `palette_command_recency`.
    pub(crate) palette_recency_tick: u64,
    /// β — `true` while this window is the foreground app window.
    /// Drives `on_paint`'s 30 Hz frame-skip when unfocused. Defaults
    /// to `true`; refreshed on WM_ACTIVATEAPP.
    pub(crate) is_window_focused: bool,
    /// `true` while this HWND holds keyboard focus. Distinct from
    /// [`Self::is_window_focused`]: switching between two continuity
    /// windows fires WM_SETFOCUS / WM_KILLFOCUS but never
    /// WM_ACTIVATEAPP. Gates the active-pane highlight so only the
    /// window the user is actually typing into advertises an active
    /// pane. Defaults to `true`; refreshed on WM_SETFOCUS /
    /// WM_KILLFOCUS. UI-thread-owned.
    pub(crate) has_keyboard_focus: bool,
    /// Whether [`Self::run`] may bring this window to the foreground
    /// when first shown. Seeded from `WindowConfig::activate_on_show`;
    /// cleared by `apply_initial_placement` when the window restores
    /// onto a non-active virtual desktop (activating it there would
    /// switch the user's desktop). UI-thread-owned, read once at show.
    pub(crate) activate_on_show: bool,
    /// β — `true` while this window is minimized. WM_PAINT returns
    /// early in that state so no compositing work happens for a
    /// surface the user can't see.
    pub(crate) is_window_minimized: bool,
    /// `true` between WM_ENTERSIZEMOVE and WM_EXITSIZEMOVE — a Win32
    /// modal sizing/move loop is in progress. While set,
    /// [`Self::refresh_client_size`] takes the cheap viewport-update
    /// path and skips the per-tick caret anchor work. The single
    /// anchor captured at WM_ENTERSIZEMOVE is restored once at
    /// WM_EXITSIZEMOVE. UI-thread-owned.
    pub(crate) is_live_resizing: bool,
    /// Screen-space no-activate popup shown while this window is the
    /// source of an active tab drag. UI-thread-owned.
    pub(crate) tab_drag_ghost_window: Option<crate::window_tab_drag_ghost::TabDragGhostWindow>,
    /// Caret anchor captured at WM_ENTERSIZEMOVE, restored once at
    /// WM_EXITSIZEMOVE so the caret line lands at its pre-drag
    /// screen y under the final projection. `None` outside a sizing
    /// loop and when no buffer is open. UI-thread-owned.
    pub(crate) resize_anchor: Option<crate::window_caret_anchor::CaretAnchor>,
    /// Test/harness probe for how often the caret-anchor capture path
    /// runs. UI-thread-owned via `Cell`; production code does not read
    /// it outside the `WM_USER + 4` harness probe.
    pub(crate) caret_anchor_capture_count: Cell<u64>,
    /// `true` if the live sizing loop produced at least one real
    /// client-size delta. Reset at WM_ENTERSIZEMOVE; set inside
    /// [`Self::refresh_client_size`] when the new size differs from
    /// the prior one; consumed at WM_EXITSIZEMOVE to decide whether
    /// a final anchor restore + invalidate is needed.
    pub(crate) resize_changed: bool,
    /// When set, the next paint snaps the focused pane's scroll to the
    /// last display row of the canonical `FrameDisplay` resolved this
    /// frame, then re-invalidates so the cold build materializes the
    /// bottom rows. Armed by `editor.move_doc_end` /
    /// `editor.extend_doc_end` because computing the exact bottom from
    /// the command thread requires reproducing the painter's projection
    /// (image reservations, fold ranges, wrap metrics) — diverging there
    /// is what caused Ctrl+End to land short of the bottom on buffers
    /// with images or stale decorations. See
    /// `crate::window_paint::cold_deferred` siblings for the snap.
    pub(crate) pending_doc_end_scroll: bool,
    /// Whether the primary caret was on-screen at the previous painted frame's end; lets layout-shift scroll anchoring skip re-targeting an already-off-screen caret. UI-thread-owned.
    pub(crate) caret_was_on_screen_prior_frame: bool,
    /// Consecutive paints the document-end snap has re-armed itself
    /// while the projection's whole-document row index was still partial
    /// (offscreen soft-wrap rows held as placeholders, so the total
    /// display-row count under-reports the true bottom). Bounds the
    /// re-snap loop so a never-completing index can't spin paint forever.
    /// Reset to `0` whenever the snap finalizes against an authoritative
    /// (non-partial) index. See `crate::window_paint::doc_end_scroll`.
    pub(crate) pending_doc_end_scroll_attempts: u8,
    /// Off-thread big-jump realization (fix A). Armed (set to
    /// the off-thread jump poll budget by a Ctrl+End / Ctrl+Home (or any
    /// far reveal) jump that lands the viewport on an
    /// unrealized region. While `> 0` and a matching projection-worker
    /// request is in flight, paint reuses the prior frame + a placeholder
    /// strip instead of inline-walking the new region on the UI thread,
    /// and re-invalidates (a cheap, input-preemptible poll) so the
    /// worker's result is picked up the moment it lands. Decremented per
    /// placeholder paint; reset to `0` on a worker hit or when the paint
    /// resolves any other way. See `worker_outcome_dispatch`.
    pub(crate) jump_offthread_polls: u8,
    /// β — paint-tick counter used to throttle background-window
    /// repaints to ~30 Hz: when unfocused, every other on_paint call
    /// returns early.
    pub(crate) background_paint_tick: u64,
    /// δ.1 — per-buffer last-edit cursor stack. Bounded ring of
    /// recently-edited [`continuity_text::Position`]s; the
    /// `editor.goto_last_edit` chord pops the most recent entry.
    pub(crate) last_edit_stack:
        HashMap<BufferId, std::collections::VecDeque<continuity_text::Position>>,
    /// β — UI-thread-owned MRU-adjacent display-map prewarm queue/cache.
    /// Holds only derived `FrameDisplay` snapshots and is cancelled on
    /// active-buffer input or revision drift.
    pub(crate) display_map_prewarm: DisplayMapPrewarm,
    /// `true` while the β idle prewarm WM_TIMER is running.
    pub(crate) display_prewarm_timer_active: bool,
    /// Per-tab swimlane state for the buffer-history visualization,
    /// keyed by [`TabId`]. Empty for regular `TabKind::Buffer` tabs.
    pub(crate) buffer_history_tabs: HashMap<TabId, BufferHistoryTab>,
    /// `GetTickCount64()` of the most recent `WM_ACTIVATEAPP(true)`.
    /// Used by the activation-grace gate that blocks nonessential
    /// idle work (spell recheck, MRU prewarm builds, spectator-pane
    /// decoration submission, …) for
    /// [`Self::ACTIVATION_GRACE_MS`] after the user returns to the
    /// window so input and paint win the first ~second. `0` means
    /// "no activation observed yet" (program startup).
    pub(crate) last_activation_tick: u64,
    /// Cached `(query, FrameDisplay)` from the most recent focused-pane
    /// paint. Soft-wrap vertical caret motion in
    /// [`crate::Window::move_line_selection`] reuses this projection
    /// when its query is `is_compatible_for_motion` with the current
    /// rope/decoration/wrap/font/fold context, so an Up/Down keystroke
    /// no longer pays an O(document) `FrameDisplay::build` per step
    /// against large (~6k-line) buffers. UI-thread-owned; cleared on
    /// the next paint when the context drifts.
    pub(crate) last_painted_frame_display:
        Option<(crate::display_prewarm_cache::PrewarmQuery, FrameDisplay)>,
    /// ε.3D — `Decorations` snapshot the last painted frame was built
    /// against. Diffing this against the current `Decorations` (after
    /// `transformed_through` shifts span bytes into the new rope's
    /// coordinates) yields the source lines whose styling actually
    /// changed; `rebuild_dirty` re-realizes only those lines instead
    /// of cold-building the whole viewport on every decoration
    /// arrival. UI-thread-owned; cleared with the frame-display
    /// cache.
    pub(crate) last_painted_decorations: Option<std::sync::Arc<continuity_decorate::Decorations>>,
    /// The worker-assigned parse revision of the `Decorations` the
    /// previous paint consumed from `decoration_cache`. Distinct from
    /// `last_painted_decorations.revision` (which is the *transformed*
    /// rope-rev label that `transformed_through` applies). When a new
    /// async parse lands in the cache its parse revision differs from
    /// this snapshot, even when the transformed label would coincide
    /// — paint detects the change and feeds
    /// `decoration_parse_advanced=true` to the classifier so the
    /// covering-cache fast path rejects the stale-styling frame.
    /// `None` until the first paint and on each cache miss.
    pub(crate) last_painted_decoration_parse_revision: Option<u64>,
    /// Cached rope-derived status-bar counts (words, non-empty lines,
    /// line-ending detection, …). Refreshed when the rope revision
    /// advances; reused verbatim on caret-motion / scroll / theme-drift
    /// / watchdog-tick paints. Keyed by [`BufferId`] so a Ctrl+Tab into
    /// a different buffer doesn't trash the prior buffer's counts and
    /// force a recompute on every switch — on a 9 k-line markdown
    /// buffer the recompute was a 450 ms WM_PAINT stall in
    /// `perf-snapshots/manual-lag_after-coalesce_20260517-232130.tsv`.
    /// See [`crate::window_status_bar::RopeStatusCounts`] for the
    /// computation. UI-thread-owned via `RefCell`.
    pub(crate) status_bar_rope_counts:
        std::cell::RefCell<HashMap<BufferId, crate::window_status_bar::RopeStatusCounts>>,
    /// ε.5b — off-UI-thread projection worker. Lazy-spawned the first
    /// paint that has a live `text_format`. `None` before then and
    /// during tests that build a `Window` without ever painting.
    /// Dropped with the window (worker thread joins on drop).
    pub(crate) projection_worker: Option<crate::projection_worker::ProjectionWorker>,
    /// ε.5b — monotonically increasing sequence number for the worker's
    /// next dispatched request. Trace events reference this so a
    /// `dispatch` line and the matching `result` line can be paired
    /// across paints.
    pub(crate) projection_request_seq: u64,
    /// ε.5e — most-recent stamp submitted by
    /// [`Window::try_dispatch_projection_worker_early`]. Compared
    /// against the next early-dispatch stamp to coalesce back-to-back
    /// edits that produce identical worker inputs (the worker would
    /// otherwise build the same projection twice). Not updated by the
    /// post-paint dispatch in `on_paint` — the worker's latest-wins
    /// recv handles paint/early-dispatch overlap.
    pub(crate) last_early_dispatch_stamp: Option<crate::projection_worker::ProjectionStamp>,
    /// Per-buffer FNV-1a content hash cache backing [`crate::pane_tree::Tab`]
    /// dirty-dot computation. Stored as `(rope_revision_seen, cur_hash)` so
    /// the per-paint walk is O(1) until the rope revision moves; recomputing
    /// the hash over the full rope used to be O(bytes) per tab per paint.
    /// UI-thread-owned via `RefCell`.
    pub(crate) tab_dirty_cache: RefCell<HashMap<BufferId, (u64, u64)>>,
    /// Pre-save content hash per buffer that has an in-flight optimistic
    /// clean mark. [`Window::mark_saved_clean`] records the previous hash
    /// here so a failed disk write (`FileIoEvent::Failed`) can roll the
    /// buffer back to dirty instead of leaving it falsely clean; the entry
    /// is removed on a successful `FileIoEvent::Saved`. UI-thread-owned.
    pub(crate) pending_save_baseline: HashMap<BufferId, u64>,
    /// Single-slot cache for the focused-pane heading-line list used by
    /// fold computation. Computing the list does a full `rope.to_string()`
    /// — caching by `(buffer, rope_revision, decoration_revision)` makes
    /// per-paint heading derivation O(1) when neither the rope nor the
    /// decorations have moved. UI-thread-owned via `RefCell`.
    pub(crate) heading_lines_cache: RefCell<Option<HeadingLinesCacheEntry>>,
    /// Single-entry cache for the focused-pane outline row list.
    /// Both paint and outline click hit-test read through this cache,
    /// keyed by `(BufferId, rope_revision, decoration_revision)`, so
    /// showing the sidebar does not rebuild headings or task progress
    /// on every frame. UI-thread-owned via `RefCell`.
    pub(crate) outline_entries_cache:
        RefCell<crate::window_outline_entries_cache::OutlineEntriesCache>,
    /// Per-pane spectator `FrameDisplay` cache. Spectator panes
    /// (every non-focused pane body) previously cold-built their
    /// projection on every paint — a 9 k-line buffer in a non-focused
    /// pane cost ~450 ms per paint while the focused pane received
    /// keystrokes. This cache holds the last painted spectator frame
    /// per pane keyed by [`crate::display_prewarm_cache::PrewarmQuery`],
    /// reused via [`crate::display_prewarm_cache::PrewarmQuery::is_compatible_for_motion`].
    /// UI-thread-owned via `RefCell` so paint can borrow the
    /// renderer and the cache simultaneously without a `&mut self`
    /// reborrow conflict. Written by spectator paint, focused-paint
    /// cache seeding, and the UI-thread projection-worker drain.
    pub(crate) spectator_frame_cache: RefCell<crate::window_spectator_cache::SpectatorFrameCache>,
    /// Mouse hit-test fallback cache. When neither
    /// `last_painted_frame_display` nor the spectator cache can
    /// satisfy `Window::resolve_hit_test_frame_display` (e.g. after a
    /// layout shortcut that drifts `wrap_width_dip`), the resolver
    /// pays for a viewport build and stores the result here so
    /// repeated mouse moves over the same buffer reuse it instead of
    /// rebuilding ~460 ms per hover.
    /// `perf-snapshots/manual-lag_after-coalesce_20260517-235814.tsv`
    /// captured several consecutive `WM_MOUSEMOVE 460 ms` events
    /// through the footnote-hover handler before the early-exit + this
    /// cache landed. The next paint may promote this frame as a
    /// rebuild source, then clears it once a paint frame supersedes
    /// it. UI-thread-owned.
    pub(crate) mouse_hit_test_frame_cache: RefCell<Option<MouseHitTestFrameCacheEntry>>,
    /// Cross-pane row-index cache. Keyed by `(BufferId, rope_rev,
    /// decoration_rev, wrap_width_dip, font_state, fold_signature)` so any
    /// pane / tab / layout showing the same buffer at the same geometry can
    /// skip the `DisplayRowIndex` walker on cold viewport builds (the walker
    /// dominates per-frame cost on large markdown buffers — ~400 ms / 9 k
    /// lines in release). See [`crate::window_row_index_cache`].
    pub(crate) row_index_cache: RefCell<crate::window_row_index_cache::RowIndexCache>,
    pub(crate) tab_session: crate::window_panes::TabSessionState, // items 8 + 18 (see window_panes)
}
