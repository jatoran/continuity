//! Process-wide event-line emitter.
//!
//! Producers above the layer graph (`command`, `app`, …) need to log
//! TSV-format events but cannot depend on `ui::paint_trace` (the layer
//! graph forbids it). This module owns its own file handle, sharing
//! only the `CONTINUITY_UI_TRACE` env var so producers land in the
//! same TSV the analyzer reads.
//!
//! Lines emitted here use the same column contract as `paint_trace`:
//! `<ms_since_start>\t<kind>\t<label>\t<duration_us>\t<details>`. The
//! analyzer treats all kinds uniformly, so callers do not need
//! coordination beyond stable label names.
//!
//! Hot-path discipline: when the env var is unset, [`is_enabled`]
//! returns `false` after one atomic load and every emitter is a no-op.
//! When enabled, file writes go through a `Mutex<BufWriter>` and are
//! flushed per line so a crash mid-action does not lose the last few
//! events.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static STDERR_FALLBACK: AtomicBool = AtomicBool::new(false);
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
    if let Some(path) = file_path {
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = FILE_SINK.set(Mutex::new(BufWriter::new(file)));
            START_TIME.get_or_init(Instant::now);
            TRACE_ENABLED.store(true, Ordering::Relaxed);
            return;
        }
    }
    if stderr_flag {
        STDERR_FALLBACK.store(true, Ordering::Relaxed);
        START_TIME.get_or_init(Instant::now);
        TRACE_ENABLED.store(true, Ordering::Relaxed);
    }
}

/// `true` when the env-gated trace is on. Cheap branch — call from any
/// hot path without measuring first.
#[inline]
#[must_use]
pub fn is_enabled() -> bool {
    ensure_initialized();
    TRACE_ENABLED.load(Ordering::Relaxed)
}

/// Adopt `t` as this crate's `START_TIME` if it is not already set.
/// Called from `crates/ui/src/paint_trace.rs::ensure_initialized`
/// right after `paint_trace` initialises its own `START_TIME`, so
/// both producers agree on `ms_since_start`. Without this,
/// `event_sink` lazily initialises its clock on the first call from
/// `command`/`app`, which may be hours into the session — emitting
/// `command_dispatch` / `smart_reopen` events at `ms_since_start=0`.
pub fn sync_start_time(t: Instant) {
    START_TIME.get_or_init(|| t);
}

fn ms_since_start() -> u128 {
    START_TIME
        .get()
        .map(|t| t.elapsed().as_micros() / 1000)
        .unwrap_or(0)
}

fn emit_line(line: String) {
    if let Some(sink) = FILE_SINK.get() {
        if let Ok(mut writer) = sink.lock() {
            let _ = writeln!(writer, "{line}");
            let _ = writer.flush();
            return;
        }
    }
    if STDERR_FALLBACK.load(Ordering::Relaxed) {
        eprintln!("{line}");
    }
}

fn sanitize_detail(detail: &str) -> String {
    detail
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            _ => ch,
        })
        .collect()
}

/// Emit one event line with no measured duration. No-op when tracing
/// is off. `label` should be a stable identifier; `detail` is
/// space-separated `key=value` pairs.
pub fn log_event(label: &str, detail: &str) {
    if !is_enabled() {
        return;
    }
    let line = format!(
        "{}\tevent\t{label}\t0\t{}",
        ms_since_start(),
        sanitize_detail(detail)
    );
    emit_line(line);
}

/// Emit one event line with a measured duration in microseconds. The
/// detail still travels with the line; stall thresholds are not
/// applied here (callers that need the stall buckets emit through
/// `paint_trace::EventScope`'s drop instead).
pub fn log_event_us(label: &str, duration_us: u64, detail: &str) {
    if !is_enabled() {
        return;
    }
    let line = format!(
        "{}\tevent\t{label}\t{duration_us}\t{}",
        ms_since_start(),
        sanitize_detail(detail)
    );
    emit_line(line);
    // Feed the running-summary registry so command_dispatch,
    // tab_close, etc. surface in the per-label percentile table.
    super::record(label, duration_us);
}
