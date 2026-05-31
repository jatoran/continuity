//! UI-thread stall trace.
//!
//! Enabled at runtime by setting either:
//! - `CONTINUITY_UI_TRACE=path/to/file.log` to write a TSV-style trace
//!   to disk (preferred for real-app focus-return triage — stderr
//!   often gets eaten by GUI process redirection), or
//! - `CONTINUITY_PAINT_TRACE=1` (legacy) for stderr output.
//!
//! When neither is set every call compiles down to a couple of
//! atomic loads and a branch — no allocations, no I/O, no clock
//! reads on the hot path.
//!
//! ## Output format
//!
//! Each line is `<ms_since_start>\t<kind>\t<label>\t<duration_us>\t<details>`.
//! `kind` is one of `event`, `paint`, `wndproc`, or `stall`. A line
//! with `kind=stall` is emitted in addition to the normal line
//! whenever any event runs longer than [`STALL_THRESHOLD_US`] — the
//! 16 ms budget the user agent flagged. Stalls over 100 ms are
//! flagged additionally as `kind=stall100`. Grep the log for
//! `\tstall100\t` and `\tstall\t` to find UI-thread blockers
//! without scrolling every line.
//!
//! The first two rows are metadata: `trace_columns` records the schema
//! version and column contract, and `trace_open` records sink/config,
//! process/build/target, cwd, and argv. Per-window state snapshots are
//! emitted by `window_trace_state.rs` as normal `event` rows so existing
//! TSV tooling keeps working.
//!
//! ## Hot-path discipline
//!
//! - `is_trace_enabled()` is the gate. If `false`, no work happens.
//! - File writes are buffered and locked behind a `Mutex<BufWriter>`;
//!   the lock is held only for the line write itself, never across
//!   timing windows.
//! - Allocations only when tracing is on. The format string itself
//!   is one `format!` per event — cheap relative to the cost of
//!   what is being measured.

use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Cross-line context (edit_seq, invalidate reason)
// lives in the sibling module so this file stays under the 600-line
// conventions cap. Re-exported so the existing
// `crate::paint_trace::<name>` import paths in other modules keep
// working without churn.
#[allow(unused_imports)]
pub(crate) use crate::paint_trace_context::{
    bind_edit_seq, current_edit_seq, next_edit_seq, EditSeqGuard,
};
use crate::paint_trace_context::{
    merge_invalidate_reason, stamp_with_edit_seq, take_invalidate_reason,
};
use crate::paint_trace_wndproc_names::wndproc_message_name;

/// Stall threshold — any event over this duration emits an extra
/// `kind=stall` line so big UI-thread blocks are visible at a
/// glance. 16 ms = one 60 Hz frame.
const STALL_THRESHOLD_US: u128 = 16_000;
/// Stall-100 threshold — anything over 100 ms is unacceptable per
/// the focus-return triage brief.
const SEVERE_STALL_THRESHOLD_US: u128 = 100_000;
/// Schema version. v4 added the chrome-overlay sub-stage split on
/// `event:renderer_draw_stages` — `chrome_overlay_line_numbers_us`,
/// `chrome_overlay_indent_guides_us`,
/// `chrome_overlay_selection_bars_us`,
/// `chrome_overlay_search_ticks_us`,
/// `chrome_overlay_block_backgrounds_us`,
/// `chrome_overlay_horizontal_rules_us`,
/// `chrome_overlay_code_copy_button_us`,
/// `chrome_overlay_minimap_us`,
/// `chrome_overlay_outline_sidebar_us`,
/// `chrome_overlay_scrollbar_us`,
/// `chrome_overlay_decoration_us`, and the validating
/// `chrome_overlay_sum_us`. The named buckets sum to within ~5 %
/// of `chrome_overlay_us`. v3 added: `event:memory_breakdown` per-
/// subsystem snapshot at flush cadence; `body_paint_us` /
/// `post_body_paint_us` on `paint:render_stats`; and `reason=` fields
/// on `wndproc:WM_PAINT`, `paint:TOTAL`, `event:row_count_walker`,
/// `event:row_index_cache action=miss`, and
/// `event:projection_worker_queue_depth`. v2 was the post-ε.7
/// baseline (schema_2 metadata + edit_seq stamping + running_summary).
const TRACE_FORMAT_VERSION: u32 = 4;

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static STDERR_FALLBACK: AtomicBool = AtomicBool::new(false);
/// When set, per-line `flush()` after `writeln!` is skipped. The
/// `BufWriter` then flushes only when its 8 KiB buffer fills (or the
/// process exits cleanly via `Drop`). Useful when investigating
/// whether per-line flush distorts WndProc / input timings — at the
/// cost of losing the last few lines on a hard crash mid-stall.
static FLUSH_PER_LINE_DISABLED: AtomicBool = AtomicBool::new(false);
static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);
/// Counts successful selection-edit applies since the last
/// `take_edits_since_paint`. Read+reset by `WM_PAINT`; sampled
/// (without reset) by other handlers that want to attach
/// `edits_since_paint=N` to their trace detail. This is the cheap
/// evidence for the "input burst starves paint" hypothesis: when
/// `edits_since_paint` is large at paint time, the UI thread
/// processed several edits between WM_PAINT delivery.
static EDITS_SINCE_PAINT: AtomicU64 = AtomicU64::new(0);
/// Wall-clock instant of the most recent `invalidate_request` call
/// that *started* a new dirty window (i.e. the previous
/// invalidate had already been satisfied by a WM_PAINT). The next
/// WM_PAINT reads + clears this and emits `paint:invalidate_to_paint`
/// with the elapsed micros. Per-paint cost: one acquire load,
/// one nanosecond, when tracing is on; no-op when off.
static OLDEST_PENDING_INVALIDATE_NS: AtomicU64 = AtomicU64::new(0);
static START_TIME: OnceLock<Instant> = OnceLock::new();
static FILE_SINK: OnceLock<Mutex<BufWriter<std::fs::File>>> = OnceLock::new();

fn ensure_initialized() {
    if TRACE_INITIALIZED.swap(true, Ordering::Relaxed) {
        return;
    }
    let file_path = std::env::var_os("CONTINUITY_UI_TRACE").map(PathBuf::from);
    let stderr_flag = std::env::var_os("CONTINUITY_PAINT_TRACE")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let no_flush = std::env::var_os("CONTINUITY_UI_TRACE_NOFLUSH")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    FLUSH_PER_LINE_DISABLED.store(no_flush, Ordering::Relaxed);
    if let Some(path) = file_path {
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let _ = FILE_SINK.set(Mutex::new(BufWriter::new(file)));
                START_TIME.get_or_init(Instant::now);
                // Hand the same `Instant` to `continuity_trace`'s
                // event-sink module so `event:command_dispatch` and
                // `event:smart_reopen` share `ms_since_start` with
                // `paint:*` rows. Without this both modules each
                // lazily initialise their own start instant and the
                // timeline becomes incoherent across producer crates.
                if let Some(t) = START_TIME.get() {
                    continuity_trace::sync_start_time(*t);
                }
                TRACE_ENABLED.store(true, Ordering::Relaxed);
                emit_trace_open(&path, no_flush, false);
                return;
            }
            Err(e) => {
                eprintln!(
                    "continuity-ui: CONTINUITY_UI_TRACE={} open failed: {e}",
                    path.display()
                );
            }
        }
    }
    if stderr_flag {
        STDERR_FALLBACK.store(true, Ordering::Relaxed);
        START_TIME.get_or_init(Instant::now);
        if let Some(t) = START_TIME.get() {
            continuity_trace::sync_start_time(*t);
        }
        TRACE_ENABLED.store(true, Ordering::Relaxed);
        emit_trace_open(&PathBuf::from("stderr"), no_flush, true);
    }
}

fn emit_trace_open(path: &Path, no_flush: bool, stderr: bool) {
    emit_line(format!(
        "0\tevent\ttrace_columns\t0\tschema={} columns=ms_since_start,kind,label,duration_us,details details=space_separated_key_value_pairs",
        TRACE_FORMAT_VERSION
    ));
    emit_line(format!(
        concat!(
            "0\tevent\ttrace_open\t0\t",
            "schema={} path={} sink={} flush_per_line={} ",
            "stall_us={} severe_us={} pid={} build={} crate_version={} ",
            "target_os={} target_arch={} cwd={} args={}"
        ),
        TRACE_FORMAT_VERSION,
        sanitize_trace_value(path.display().to_string()),
        if stderr { "stderr" } else { "file" },
        !no_flush,
        STALL_THRESHOLD_US,
        SEVERE_STALL_THRESHOLD_US,
        std::process::id(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::current_dir()
            .map(|path| sanitize_trace_value(path.display().to_string()))
            .unwrap_or_else(|_| "unknown".to_string()),
        sanitize_trace_value(std::env::args().collect::<Vec<_>>().join("|")),
    ));
}

fn sanitize_trace_value(value: String) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            _ => ch,
        })
        .collect()
}

fn emit_line(line: String) {
    if let Some(sink) = FILE_SINK.get() {
        if let Ok(mut writer) = sink.lock() {
            let _ = writeln!(writer, "{line}");
            if !FLUSH_PER_LINE_DISABLED.load(Ordering::Relaxed) {
                // Flush per-line so a crash mid-stall doesn't lose
                // the last events. Hot-path overhead is acceptable
                // because the gate is off in production. Opt-out via
                // `CONTINUITY_UI_TRACE_NOFLUSH=1` when investigating
                // whether the flush itself distorts WndProc timings.
                let _ = writer.flush();
            }
            return;
        }
    }
    if STDERR_FALLBACK.load(Ordering::Relaxed) {
        eprintln!("{line}");
    }
}

/// Increment the edit-apply counter. Called once per successful
/// selection-edit landing (`dispatch_selection_edit`'s
/// `edit_apply_result` site).
#[inline]
pub(crate) fn note_edit_applied() {
    EDITS_SINCE_PAINT.fetch_add(1, Ordering::Relaxed);
}

/// Sample the edit-since-paint counter without resetting it.
/// Suitable for attaching `edits_since_paint=N` to a wndproc detail
/// line.
#[inline]
pub(crate) fn edits_since_paint() -> u64 {
    EDITS_SINCE_PAINT.load(Ordering::Relaxed)
}

/// Take + reset the counter. Called by `WM_PAINT`'s prologue so the
/// next paint scope can report how many edits landed since the
/// previous paint.
#[inline]
pub(crate) fn take_edits_since_paint() -> u64 {
    EDITS_SINCE_PAINT.swap(0, Ordering::Relaxed)
}

/// Note an invalidate with a call-site reason. The oldest timestamp is
/// preserved for invalidate-to-paint latency; the reason keeps the
/// highest-priority pending trigger so coalesced `WM_PAINT`s still name
/// the strongest cause.
#[inline]
pub(crate) fn note_invalidate_request_with_reason(reason: &'static str) {
    merge_invalidate_reason(reason);
    if !is_trace_enabled() {
        return;
    }
    if let Some(t) = START_TIME.get() {
        let now_ns = t.elapsed().as_nanos() as u64;
        let _ = OLDEST_PENDING_INVALIDATE_NS.compare_exchange(
            0,
            now_ns.max(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
}

/// WM_PAINT prologue: returns the `Some(elapsed_us)` since the
/// oldest pending invalidate request was armed, or `None` if no
/// pending request was tracked.
#[inline]
pub(crate) fn take_invalidate_to_paint_us() -> Option<u128> {
    let stamp = OLDEST_PENDING_INVALIDATE_NS.swap(0, Ordering::Relaxed);
    if stamp == 0 {
        return None;
    }
    let now = START_TIME.get()?.elapsed().as_nanos() as u64;
    Some(u128::from(now.saturating_sub(stamp)) / 1000)
}

fn ms_since_start() -> u128 {
    START_TIME
        .get()
        .map(|t| t.elapsed().as_micros() / 1000)
        .unwrap_or(0)
}

/// True when the env-gated trace is on. Cheap branch — call from
/// any hot path without measuring first.
#[inline]
pub(crate) fn is_trace_enabled() -> bool {
    ensure_initialized();
    TRACE_ENABLED.load(Ordering::Relaxed)
}

fn emit_event(kind: &str, label: &str, duration: Duration, details: &str) {
    let us = duration.as_micros();
    let stamped = stamp_with_edit_seq(details);
    let line = format!("{}\t{kind}\t{label}\t{us}\t{stamped}", ms_since_start(),);
    emit_line(line);
    if us >= SEVERE_STALL_THRESHOLD_US {
        emit_line(format!(
            "{}\tstall100\t{label}\t{us}\t{stamped}",
            ms_since_start(),
        ));
        if let Some(stack_detail) =
            crate::paint_trace_stack::stall_stack_detail(label, us, stamped.as_ref())
        {
            emit_line(format!(
                "{}\tevent\tstall100_stack\t0\t{stack_detail}",
                ms_since_start(),
            ));
        }
    } else if us >= STALL_THRESHOLD_US {
        emit_line(format!(
            "{}\tstall\t{label}\t{us}\t{stamped}",
            ms_since_start(),
        ));
    }
    // Feed the running-summary registry. `record` is gated by the same
    // env-vars as the rest of the tracer via the `is_trace_enabled`
    // checks at every caller; this site is only reached when tracing
    // is on.
    let us_u64 = u64::try_from(us).unwrap_or(u64::MAX);
    crate::paint_trace_summary::record(label, us_u64);
}

/// Per-paint scope. Cheap to construct; no I/O until
/// [`PaintTrace::finish`] when tracing is enabled.
pub(crate) struct PaintTrace {
    frame: Option<u64>,
    started: Option<Instant>,
    last_mark: Cell<Option<Instant>>,
    buffer_lines: u32,
    revision: u64,
    /// `reason=` field for `paint:TOTAL`. Captured from the pending-
    /// invalidate thread-local at `new`; `None` when paint was
    /// OS-driven (no `invalidate_rect` call) or the invalidate site
    /// didn't carry a reason.
    invalidate_reason: Option<&'static str>,
}

impl PaintTrace {
    /// Start a new paint trace. Returns an inert handle when tracing
    /// is disabled — no clock reads, no allocations, no output.
    /// Captures the pending invalidate reason so `finish` can stamp
    /// `reason=…` into `paint:TOTAL`.
    #[must_use]
    pub(crate) fn new(buffer_lines: u32, revision: u64) -> Self {
        let invalidate_reason = take_invalidate_reason();
        if is_trace_enabled() {
            let frame = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);
            let now = Instant::now();
            Self {
                frame: Some(frame),
                started: Some(now),
                last_mark: Cell::new(Some(now)),
                buffer_lines,
                revision,
                invalidate_reason,
            }
        } else {
            Self {
                frame: None,
                started: None,
                last_mark: Cell::new(None),
                buffer_lines,
                revision,
                invalidate_reason,
            }
        }
    }

    /// Record the duration of a stage since the last `mark` (or
    /// trace start). No-op when tracing is disabled.
    pub(crate) fn mark(&self, stage: &str) {
        if let Some(last) = self.last_mark.get() {
            let now = Instant::now();
            let elapsed = now.duration_since(last);
            let detail = format!(
                "frame={} lines={} rev={}",
                self.frame.unwrap_or(0),
                self.buffer_lines,
                self.revision,
            );
            emit_event("paint", stage, elapsed, &detail);
            self.last_mark.set(Some(now));
        }
    }

    /// Emit `paint:<stage>` with the cumulative duration since
    /// [`Self::new`] returned — *not* since the most recent
    /// [`Self::mark`] call. The inter-mark cursor is left
    /// untouched, so interspersing a `mark_since_start` between
    /// regular `mark` calls does not perturb the existing stage
    /// timings.
    ///
    /// Used by the ε.5g UI-thread frame-display-readiness perf
    /// gate, which needs one sample per paint from `WM_PAINT`
    /// entry to the moment the `FrameDisplay` is in hand,
    /// independent of how many intermediate stage marks fired.
    pub(crate) fn mark_since_start(&self, stage: &str, extra: &str) {
        if let Some(started) = self.started {
            let detail = format!(
                "frame={} lines={} rev={} {extra}",
                self.frame.unwrap_or(0),
                self.buffer_lines,
                self.revision,
            );
            emit_event("paint", stage, started.elapsed(), &detail);
        }
    }

    /// Pending invalidate reason captured for this paint, if tracing
    /// is enabled and the paint came from `invalidate_with_reason`.
    #[must_use]
    pub(crate) fn invalidate_reason(&self) -> Option<&'static str> {
        self.invalidate_reason
    }

    /// Print the trailing frame summary. No-op when tracing is
    /// disabled. Includes `reason=<invalidate_reason>` (or
    /// `reason=os_or_unknown` for OS-driven paints with no pending
    /// invalidate recorded).
    pub(crate) fn finish(self, extra: &str) {
        if let (Some(started), Some(frame)) = (self.started, self.frame) {
            let total = started.elapsed();
            let reason = self.invalidate_reason.unwrap_or("os_or_unknown");
            let detail = if extra.is_empty() {
                format!(
                    "frame={frame} lines={} rev={} reason={reason}",
                    self.buffer_lines, self.revision,
                )
            } else {
                format!(
                    "frame={frame} lines={} rev={} reason={reason} {extra}",
                    self.buffer_lines, self.revision,
                )
            };
            emit_event("paint", "TOTAL", total, &detail);
        }
    }
}

/// Timed scope for a discrete UI event (focus toggle, activation
/// step, idle prewarm tick, …). Drops cheaply when tracing is off;
/// when on, emits `event:<name>=<duration>` (and a `stall` line when
/// the event was over budget) at scope end so the line lands close
/// to the work it caused.
pub(crate) struct EventScope {
    label: &'static str,
    detail: String,
    started: Option<Instant>,
}

impl EventScope {
    /// Open a new scope. The trailing line prints on drop.
    #[must_use]
    pub(crate) fn new(label: &'static str) -> Self {
        let started = is_trace_enabled().then(Instant::now);
        Self {
            label,
            detail: String::new(),
            started,
        }
    }

    /// Attach arbitrary text to the scope's emitted line. Only
    /// formats when tracing is on.
    #[allow(dead_code)]
    pub(crate) fn with_detail(label: &'static str, detail: String) -> Self {
        let started = is_trace_enabled().then(Instant::now);
        Self {
            label,
            detail: if started.is_some() {
                detail
            } else {
                String::new()
            },
            started,
        }
    }

    /// Replace the scope detail before it emits on drop.
    pub(crate) fn set_detail(&mut self, detail: String) {
        if self.started.is_some() {
            self.detail = detail;
        }
    }
}

impl Drop for EventScope {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            emit_event("event", self.label, started.elapsed(), &self.detail);
        }
    }
}

/// Drain the input-burst counters and log the per-paint `paint_prologue`
/// event ("edits queued behind the paint" + "oldest-dirty-invalidate to
/// paint" delay). No-op when tracing is off (one relaxed swap + branch).
pub(crate) fn log_paint_prologue() {
    if !is_trace_enabled() {
        return;
    }
    let edits = take_edits_since_paint();
    let invalidate_to_paint = take_invalidate_to_paint_us();
    log_event(
        "paint_prologue",
        &format!(
            "edits_since_paint={edits} invalidate_to_paint_us={}",
            invalidate_to_paint.map_or(-1i64, |v| v as i64),
        ),
    );
}

/// One-line event log without a measured duration. No-op when
/// tracing is off.
pub(crate) fn log_event(label: &str, detail: &str) {
    if is_trace_enabled() {
        let stamped = stamp_with_edit_seq(detail);
        let line = format!("{}\tevent\t{label}\t0\t{stamped}", ms_since_start());
        emit_line(line);
    }
}

/// Scope for one wndproc message dispatch. Records the message
/// name, duration, and timer/key code where relevant. Stalls
/// > 16 ms emit an extra `stall` line; > 100 ms emits `stall100`.
pub(crate) struct WndprocScope {
    name: String,
    detail: String,
    started: Option<Instant>,
}

impl WndprocScope {
    /// Open a new wndproc scope. `wparam_detail` is a free-form
    /// string the caller passes for message-specific context
    /// (timer id, key code, activation flag, …).
    #[must_use]
    pub(crate) fn new(msg: u32, wparam_detail: &str) -> Self {
        if !is_trace_enabled() {
            return Self {
                name: String::new(),
                detail: String::new(),
                started: None,
            };
        }
        Self {
            name: wndproc_message_name(msg),
            detail: wparam_detail.to_string(),
            started: Some(Instant::now()),
        }
    }
}

impl Drop for WndprocScope {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            emit_event("wndproc", &self.name, started.elapsed(), &self.detail);
        }
    }
}
