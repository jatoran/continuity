//! Core-thread instrumentation shim.
//!
//! Mirrors `ui::paint_trace`'s gate and output format so a single
//! `CONTINUITY_UI_TRACE=<path>` env var captures UI- and core-thread
//! events into the same TSV. The persist crate has an identical
//! sibling module. All three open the trace file independently in
//! `append` mode; line-level writes are atomic on Windows and POSIX
//! for sub-PIPE_BUF lines, so cross-crate output interleaves cleanly
//! at the line boundary without needing a shared sink.
//!
//! Each line is `<ms_since_start>\t<kind>\t<label>\t<duration_us>\t<details>`,
//! matching the existing format consumed by perf scripts.

use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

thread_local! {
    /// Bound by [`bind_edit_seq`] for the duration of one
    /// `ApplyEdit` / `ApplySelectionEdit` message handler. Stamped
    /// into every emitted trace line so the same `edit_seq=N` shows
    /// up alongside the UI-side and (when added) persist-side
    /// scopes for the same edit.
    static CURRENT_EDIT_SEQ: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Look up the currently-bound edit sequence number.
#[inline]
pub(crate) fn current_edit_seq() -> Option<u64> {
    CURRENT_EDIT_SEQ.with(|c| c.get())
}

/// Bind `seq` until the returned guard drops. Nested binds save and
/// restore so an edit triggering a nested edit on the same thread
/// produces the right correlation labels.
#[must_use = "the guard restores the previous edit_seq on drop"]
pub(crate) fn bind_edit_seq(seq: u64) -> EditSeqGuard {
    let previous = CURRENT_EDIT_SEQ.with(|c| c.replace(Some(seq)));
    EditSeqGuard { previous }
}

pub(crate) struct EditSeqGuard {
    previous: Option<u64>,
}

impl Drop for EditSeqGuard {
    fn drop(&mut self) {
        let prev = self.previous;
        CURRENT_EDIT_SEQ.with(|c| c.set(prev));
    }
}

fn stamp_with_edit_seq(detail: &str) -> std::borrow::Cow<'_, str> {
    match current_edit_seq() {
        Some(seq) if !detail.is_empty() => {
            std::borrow::Cow::Owned(format!("edit_seq={seq} {detail}"))
        }
        Some(seq) => std::borrow::Cow::Owned(format!("edit_seq={seq}")),
        None => std::borrow::Cow::Borrowed(detail),
    }
}

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static START_TIME: OnceLock<Instant> = OnceLock::new();
static FILE_SINK: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

fn ensure_initialized() {
    if TRACE_INITIALIZED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(path) = std::env::var_os("CONTINUITY_UI_TRACE").map(PathBuf::from) else {
        return;
    };
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = FILE_SINK.set(Mutex::new(file));
    START_TIME.get_or_init(Instant::now);
    TRACE_ENABLED.store(true, Ordering::Relaxed);
}

/// Cheap branch — when false the caller skips all formatting and
/// timer work. One atomic load on the hot path when tracing is off.
#[inline]
pub(crate) fn is_trace_enabled() -> bool {
    ensure_initialized();
    TRACE_ENABLED.load(Ordering::Relaxed)
}

fn ms_since_start() -> u128 {
    START_TIME
        .get()
        .map(|t| t.elapsed().as_micros() / 1000)
        .unwrap_or(0)
}

fn emit_line(line: &str) {
    if let Some(sink) = FILE_SINK.get() {
        if let Ok(mut file) = sink.lock() {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// One-line event log with explicit duration. No-op when tracing is off.
#[allow(dead_code)]
pub(crate) fn log_event(label: &str, duration_us: u128, detail: &str) {
    if !is_trace_enabled() {
        return;
    }
    let stamped = stamp_with_edit_seq(detail);
    let line = format!(
        "{}\tevent\t{label}\t{duration_us}\t{stamped}",
        ms_since_start()
    );
    emit_line(&line);
    let us = u64::try_from(duration_us).unwrap_or(u64::MAX);
    continuity_trace::record(label, us);
}

/// Scope timer — emits `event:<label>\t<elapsed_us>\t<detail>` on
/// drop. Constructing it when tracing is off costs one atomic load
/// and a no-op `Drop`; it does not read the clock or allocate.
pub(crate) struct Scope {
    label: &'static str,
    detail: String,
    started: Option<Instant>,
}

impl Scope {
    pub(crate) fn new(label: &'static str) -> Self {
        let started = is_trace_enabled().then(Instant::now);
        Self {
            label,
            detail: String::new(),
            started,
        }
    }

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

    /// Attach (or replace) the detail string. Cheap no-op when off.
    #[allow(dead_code)]
    pub(crate) fn set_detail(&mut self, detail: String) {
        if self.started.is_some() {
            self.detail = detail;
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            let elapsed = started.elapsed();
            let us = elapsed.as_micros();
            let stamped = stamp_with_edit_seq(&self.detail);
            let line = format!(
                "{}\tevent\t{}\t{us}\t{stamped}",
                ms_since_start(),
                self.label,
            );
            emit_line(&line);
            let us_u64 = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
            continuity_trace::record(self.label, us_u64);
        }
    }
}
