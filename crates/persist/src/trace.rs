//! Persist-thread instrumentation shim.
//!
//! Sibling of `continuity_core::trace`; see the docstring there for
//! the cross-crate trace model. All three crates open the same
//! `CONTINUITY_UI_TRACE` file path in `append` mode and rely on the
//! OS to interleave lines atomically.

use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

thread_local! {
    /// Bound by [`bind_edit_seq`] for the duration of one persist
    /// message handler that processes a specific edit. Stamped into
    /// every emitted persist-thread trace line so the same
    /// `edit_seq=N` appears alongside the UI and core scopes for the
    /// same edit.
    static CURRENT_EDIT_SEQ: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Bind `seq` until the returned guard drops.
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
    match CURRENT_EDIT_SEQ.with(|c| c.get()) {
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

pub(crate) struct Scope {
    label: &'static str,
    detail: String,
    started: Option<Instant>,
}

impl Scope {
    #[allow(dead_code)]
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
