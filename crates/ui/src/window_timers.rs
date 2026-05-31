//! Per-window `WM_TIMER` ids and cadences plus the wheel constants used
//! by the runtime helpers.
//!
//! Pulled out of [`crate::window`] so the timer surface lives behind one
//! header instead of getting interleaved with the `Window` struct
//! definition and message-pump glue.
//!
//! **Thread ownership**: read-only constants, but `SetTimer`/`KillTimer`
//! must be called from the owning window's UI thread.

/// Periodic timer cadence for the smooth-scroll animator. ~60 FPS.
pub(crate) const SCROLL_ANIM_TIMER_ID: usize = 1;
/// Tick interval for the smooth-scroll animator in milliseconds.
pub(crate) const SCROLL_ANIM_TIMER_MS: u32 = 16;
/// Caret-blink `WM_TIMER` id; cadence comes from `view_options.caret_blink_ms`.
pub(crate) const CARET_BLINK_TIMER_ID: usize = 2;
/// Phase-12 live-reload poll `WM_TIMER` id; drains the `ConfigEvent`
/// channel. Public so the §C5 Win32 e2e harness can synchronously
/// fire `WM_TIMER` with this id to force a drain without waiting on
/// the OS timer to tick.
pub const CONFIG_POLL_TIMER_ID: usize = 3;
/// Cadence for the live-reload poll. 250 ms is comfortably faster than
/// human-perceptible save → reload latency without burning CPU.
pub(crate) const CONFIG_POLL_TIMER_MS: u32 = 250;
/// Phase-15 file-I/O completion poll timer id.
pub(crate) const FILE_IO_TIMER_ID: usize = 4;
/// File-I/O completion poll cadence.
pub(crate) const FILE_IO_TIMER_MS: u32 = 100;
/// Phase-17 debounced window-state save timer id. Coarse pane-tree, tab,
/// placement, and view-state changes arm this timer; the next fire commits
/// `save_window_placement_state()` so a process crash mid-session doesn't lose
/// layout. Sessions still save synchronously on `WM_DESTROY`.
pub(crate) const STATE_SAVE_TIMER_ID: usize = 5;
/// Debounce window for `STATE_SAVE_TIMER_ID`. 250 ms is short enough to
/// land before the user's eye notices a layout change but long enough to
/// coalesce bursts (Ctrl+Alt+5 rebuilding a 2×2 grid is ~6 mutations).
pub(crate) const STATE_SAVE_DEBOUNCE_MS: u32 = 250;
/// Base lines scrolled per mouse-wheel notch before the user speed multiplier.
pub(crate) const WHEEL_LINES_PER_NOTCH: f32 = 3.0;
/// Phase-I2 metrics-buffer repaint timer id. Armed while the dedicated
/// metrics buffer is the focused tab; flushed on focus loss.
pub(crate) const METRICS_REPAINT_TIMER_ID: usize = 6;
/// 1 Hz cadence per spec §I2 ("throttled to 1 Hz while active").
pub(crate) const METRICS_REPAINT_TIMER_MS: u32 = 1_000;
/// §H6 Ctrl+Tab hold-detection timer id. Armed on the first Ctrl+Tab
/// press, fires after [`TAB_OVERLAY_HOLD_TIMER_MS`] to open the
/// positional tab switcher. Cancelled by an early Ctrl release (fast
/// swap, no overlay) or by the overlay opening for any other reason.
pub(crate) const TAB_OVERLAY_HOLD_TIMER_ID: usize = 7;
/// Spec §6 override: the overlay appears after a 600 ms `Ctrl` hold.
/// Releases before the timer fires never reach the overlay.
pub(crate) const TAB_OVERLAY_HOLD_TIMER_MS: u32 = 600;
/// Footnote hover-peek dwell timer id. One-shot; armed on reference enter.
pub(crate) const FOOTNOTE_HOVER_TIMER_ID: usize = 8;
/// Dwell before a footnote reference opens its passive peek panel.
pub(crate) const FOOTNOTE_HOVER_TIMER_MS: u32 = 300;
/// α.0 shared motion timer id. Drives overlay/chrome/status/chord-HUD
/// frames and delayed dwell checks.
pub(crate) const MOTION_TIMER_ID: usize = 10;
/// Decoration worker watchdog event poll timer id.
pub(crate) const DECORATION_WATCHDOG_TIMER_ID: usize = 9;
/// Poll cadence for decoration worker restart events and status-chip fade.
pub(crate) const DECORATION_WATCHDOG_TIMER_MS: u32 = 250;
/// β prewarm timer id. The handler only performs work after the idle
/// detector confirms no input, persistence backlog, or repaint is pending.
pub(crate) const DISPLAY_PREWARM_TIMER_ID: usize = 11;
/// β prewarm cadence. The tick is cheap when not idle and processes at
/// most one display-map stage when idle.
pub(crate) const DISPLAY_PREWARM_TIMER_MS: u32 = 50;
/// Image-animation tick. Advances every animated entry's `frame_index`
/// in the renderer's image cache and invalidates the window on change.
/// Armed only while at least one animated image is resident; auto-
/// disarms when the cache drops back to all-static entries.
pub(crate) const IMAGE_ANIMATION_TIMER_ID: usize = 12;
/// Image-animation tick cadence. 50 ms matches
/// `continuity_render::image_cache::MIN_FRAME_DELAY_MS` so every
/// frame transition lands on the next tick.
pub(crate) const IMAGE_ANIMATION_TIMER_MS: u32 = 50;
/// Running-summary trace flush timer id. Periodically iterates the
/// per-label histogram registry in [`crate::paint_trace_summary`] and
/// emits one `event:running_summary` line per label. Armed only when
/// `CONTINUITY_UI_TRACE` is set; cadence comes from
/// `CONTINUITY_TRACE_SUMMARY_MS` (default 2000, min 250, off when 0).
pub(crate) const TRACE_SUMMARY_TIMER_ID: usize = 13;
/// Text-selection drag autoscroll timer id. Armed only while the left
/// button is held and the cursor sits past the focused body top/bottom
/// dead band.
pub(crate) const MOUSE_DRAG_AUTOSCROLL_TIMER_ID: usize = 14;
/// Tick interval for text-selection drag autoscroll.
pub(crate) const MOUSE_DRAG_AUTOSCROLL_TIMER_MS: u32 = 16;
/// Code-block copy-button "Copied" feedback timer id. One-shot; armed
/// after a successful (or failed) clipboard write so the button reverts
/// to its idle state once the user has seen the confirmation.
pub(crate) const CODE_COPY_FEEDBACK_TIMER_ID: usize = 15;
/// Duration (ms) the "Copied" / "Failed" feedback remains visible on the
/// copy button after a click. The spec called for ~800 ms; the value
/// settled at 1500 ms after user feedback — at 800 ms the icon swap +
/// background tint read as janky because the eye barely registered the
/// confirmation before it reverted. 1500 ms keeps the chip stable
/// long enough to be unambiguously seen, still short enough that
/// lingering after the user moves on doesn't feel sticky.
pub(crate) const CODE_COPY_FEEDBACK_TIMER_MS: u32 = 1500;
