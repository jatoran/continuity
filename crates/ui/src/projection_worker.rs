// ε.5 ships the worker foundation only; the production paint path
// keeps building projections inline this slice. Every public type
// reads "never used" until the ε.5 integration slice wires
// `Window::on_paint` to dispatch + validate worker results.
#![allow(dead_code)]
//! ε.5 — off-UI-thread viewport projection worker.
//!
//! **Thread ownership.** One worker thread per [`ProjectionWorker`]. The
//! UI thread is the sole producer ([`ProjectionWorker::submit`]) and the
//! sole consumer ([`ProjectionWorker::take_latest_result`]). The worker
//! thread reads request payloads and appends to a bounded result queue.
//!
//! **Per-pane latest-wins request channel.** Submitted requests are
//! coalesced by target pane: when the worker wakes, it drains available
//! requests and keeps the most recent request for each pane. A typing
//! burst in the focused pane still collapses to one build, while a
//! layout-template prewarm can retain one request per live pane.
//!
//! **Bounded result queue.** Results live in a `Mutex<VecDeque<…>>`
//! shared with the UI thread. The queue is bounded to the command
//! capacity; oldest unread results are dropped only if paint stops
//! draining entirely. The UI thread validates every result against the
//! current paint inputs via [`ProjectionStamp`] before painting — stale
//! results are dropped, not displayed, because the contract of ε is
//! *"the display map is allowed to be incomplete, stale, or partially
//! realized — but never wrong"*.
//!
//! **DirectWrite thread-safety.** `IDWriteFactory` and immutable
//! `IDWriteTextFormat` objects are documented as thread-safe in
//! <https://learn.microsoft.com/en-us/windows/win32/directwrite/multi-threading>.
//! The `windows-rs` 0.59 crate does not auto-impl `Send + Sync` for COM
//! interface handles (conservative default), so [`SendCom`] wraps them
//! with `unsafe impl Send + Sync`. Per-build `IDWriteTextLayout` objects
//! are created and dropped on the worker thread without crossing back.
//!
//! **Foundation slice only.** This module ships the worker plumbing,
//! request/result schema, and unit tests. The integration that swaps
//! `Window::on_paint` to dispatch to the worker is intentionally
//! deferred (see roadmap_v4.md ε.5 status). The schema is sized so the
//! integration slice can wire it through without changing the worker
//! contract.
//!
//! Topical sub-modules:
//!
//! - [`stamp`] — [`ProjectionStamp`] + [`StampMismatchField`] diff trace
//! - [`schema`] — [`ProjectionPlan`] / [`ProjectionRequest`] / [`ProjectionResult`]
//! - [`measure`] — [`MeasureMode`] backend + [`SendCom`] COM wrapper
//! - [`worker_loop`] — worker-thread receive/coalesce/build/publish loop

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, Sender};
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;

use continuity_display_map::{SegmentCache, WrapCache};
use continuity_layout::RunCache;

use crate::pane_tree::PaneId;

mod measure;
mod schema;
mod stamp;
mod worker_loop;

#[cfg(test)]
mod tests;

pub(crate) use measure::{MeasureMode, SendCom};
pub(crate) use schema::{ProjectionPlan, ProjectionRequest, ProjectionResult, WorkerFontMetrics};
pub(crate) use stamp::{ProjectionStamp, StampMismatchField};

use self::worker_loop::worker_loop;

#[derive(Clone)]
pub(super) struct PendingProjectionRequest {
    pub seq: u64,
    pub target_pane: PaneId,
    pub stamp: ProjectionStamp,
    /// Submission reason captured at [`ProjectionWorker::submit_with_reason`]
    /// time. Carried alongside the pending entry so paint-time wait
    /// classification (P0.8.3) can distinguish a `layout_change` /
    /// `focus_change` in-flight build (extended bounded-wait budget
    /// eligible) from any other category (`early_dispatch`,
    /// `paint_epilogue`, `paint_resubmit`, …). Stored as the same
    /// `&'static str` the queue-depth trace emits so callers compare by
    /// literal value.
    pub reason: &'static str,
}

pub(super) type PendingProjectionRequests = Arc<Mutex<VecDeque<PendingProjectionRequest>>>;

pub(crate) const PAINT_PARTIAL_FILL_REASON: &str = "paint_partial_fill";

/// Bounded result queue plus the condvar the worker thread signals
/// after writing into it. Paint can wait on the condvar for a bounded
/// window when the worker is mid-build for the live paint stamp — far
/// cheaper than the inline cold build the fallthrough would pay
/// otherwise (see [`ProjectionWorker::wait_for_result_publication`]).
///
/// The `Mutex` is the single rendezvous point between the worker (sole
/// writer) and the UI thread (sole reader). The condvar's monitor is the
/// same mutex; sleeping releases it so the worker can publish without
/// contention.
pub(super) struct ResultCell {
    /// Owning thread on read: the UI thread of one window.
    /// Owning thread on write: the projection worker thread spawned by
    /// [`ProjectionWorker::spawn_with_caches`].
    pub(super) results: Mutex<VecDeque<ProjectionResult>>,
    /// Signaled by the worker after every publish; awaited by paint via
    /// [`ProjectionWorker::wait_for_result_publication`].
    pub(super) publication: Condvar,
}

impl ResultCell {
    fn new() -> Self {
        Self {
            results: Mutex::new(VecDeque::new()),
            publication: Condvar::new(),
        }
    }
}

/// Off-UI-thread viewport projection worker.
pub(crate) struct ProjectionWorker {
    cmd_tx: Option<Sender<ProjectionRequest>>,
    latest_result: Arc<ResultCell>,
    /// Shared queue/in-flight bookkeeping. UI thread pushes after a
    /// submit; the worker thread removes coalesced and completed
    /// requests. The mutex protects only this tiny diagnostic/dedupe
    /// list, not projection builds or result delivery.
    pending_requests: PendingProjectionRequests,
    wrap_cache: Arc<WrapCache>,
    segment_cache: Arc<SegmentCache>,
    processed_count: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

/// Channel capacity for queued requests. The worker drains down to one
/// per iteration; this bound only matters when the UI thread submits
/// faster than the channel can absorb between worker pop + drain. 64 is
/// generous given each request is ~Arc-clones-and-an-enum.
const COMMAND_CHANNEL_CAPACITY: usize = 64;
const RESULT_QUEUE_CAPACITY: usize = COMMAND_CHANNEL_CAPACITY;

impl ProjectionWorker {
    /// Spawn a worker thread using `measure_mode`.
    #[must_use]
    pub(crate) fn spawn(measure_mode: MeasureMode) -> Self {
        Self::spawn_with_caches(
            measure_mode,
            Arc::new(WrapCache::default()),
            Arc::new(SegmentCache::default()),
        )
    }

    /// Spawn a worker thread sharing row-count caches with the inline
    /// fallback path.
    #[must_use]
    pub(crate) fn spawn_with_caches(
        measure_mode: MeasureMode,
        wrap_cache: Arc<WrapCache>,
        segment_cache: Arc<SegmentCache>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = bounded::<ProjectionRequest>(COMMAND_CHANNEL_CAPACITY);
        let latest_result: Arc<ResultCell> = Arc::new(ResultCell::new());
        let pending_requests: PendingProjectionRequests = Arc::new(Mutex::new(VecDeque::new()));
        let processed_count: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let latest_for_thread = Arc::clone(&latest_result);
        let pending_for_thread = Arc::clone(&pending_requests);
        let processed_for_thread = Arc::clone(&processed_count);
        let wrap_for_thread = Arc::clone(&wrap_cache);
        let segment_for_thread = Arc::clone(&segment_cache);
        let thread = thread::Builder::new()
            .name("projection-worker".into())
            .spawn(move || {
                worker_loop(
                    cmd_rx,
                    measure_mode,
                    latest_for_thread,
                    pending_for_thread,
                    wrap_for_thread,
                    segment_for_thread,
                    processed_for_thread,
                );
            })
            .expect("invariant: spawning projection worker should not fail");
        Self {
            cmd_tx: Some(cmd_tx),
            latest_result,
            pending_requests,
            wrap_cache,
            segment_cache,
            processed_count,
            thread: Some(thread),
        }
    }

    /// Build the production [`MeasureMode::DirectWrite`] from the
    /// UI-thread DirectWrite factory. Font size + text format are NOT
    /// baked here; they arrive per request via [`WorkerFontMetrics`],
    /// so a font change needs no worker respawn (RC1 stale-font fix).
    /// Cloning the factory COM handle is a single atomic `AddRef`.
    #[must_use]
    pub(crate) fn direct_write_mode(
        factory: IDWriteFactory,
        run_cache: Arc<RunCache>,
    ) -> MeasureMode {
        MeasureMode::DirectWrite {
            // SAFETY: IDWriteFactory is documented as thread-safe by
            // Microsoft DirectWrite docs.
            factory: unsafe { SendCom::new(factory) },
            run_cache,
            locale: crate::window::FONT_LOCALE,
        }
    }

    /// Submit a request. Older queued requests for the same worker are
    /// silently dropped when the worker drains the queue between
    /// builds. Returns `false` only after `shutdown`/`drop` has run.
    pub(crate) fn submit(&self, request: ProjectionRequest) -> bool {
        self.submit_with_reason(request, "unspecified")
    }

    /// As [`Self::submit`] but tags the queue-depth trace line with a
    /// dispatch reason (`paint_epilogue` / `early_dispatch_<funnel>` /
    /// `selection_edit` / …). Surfaces in
    /// `event:projection_worker_queue_depth` so the trace consumer can
    /// see which submission path is filling the channel.
    pub(crate) fn submit_with_reason(
        &self,
        request: ProjectionRequest,
        reason: &'static str,
    ) -> bool {
        let Some(tx) = self.cmd_tx.as_ref() else {
            return false;
        };
        let seq = request.seq;
        let target_pane = request.target_pane;
        let stamp = request.stamp.clone();
        self.push_pending_request(PendingProjectionRequest {
            seq,
            target_pane,
            stamp,
            reason,
        });
        let result = match tx.try_send(request) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                self.remove_pending_request(seq);
                false
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                self.remove_pending_request(seq);
                false
            }
        };
        if crate::paint_trace::is_trace_enabled() {
            crate::paint_trace::log_event(
                "projection_worker_queue_depth",
                &format!(
                    "depth={} capacity={} accepted={} reason={reason}",
                    tx.len(),
                    COMMAND_CHANNEL_CAPACITY,
                    result
                ),
            );
        }
        result
    }

    /// `true` when this exact stamp is already queued or currently
    /// being built by the worker.
    #[must_use]
    pub(crate) fn has_pending_stamp(&self, stamp: &ProjectionStamp) -> bool {
        self.pending_requests
            .lock()
            .ok()
            .is_some_and(|pending| pending.iter().any(|entry| entry.stamp.eq(stamp)))
    }

    /// `true` when this exact pane/stamp request is already queued or
    /// currently being built by the worker.
    #[must_use]
    pub(crate) fn has_pending_target_stamp(
        &self,
        target_pane: PaneId,
        stamp: &ProjectionStamp,
    ) -> bool {
        self.pending_requests.lock().ok().is_some_and(|pending| {
            pending
                .iter()
                .any(|entry| entry.target_pane == target_pane && entry.stamp.eq(stamp))
        })
    }

    /// `true` when a background fill for the same buffer/font/wrap
    /// prefix is already queued or currently being built. The fill may
    /// be for the current rope revision or an older one; either way we
    /// keep at most one partial-fill request in flight so routine paints
    /// do not saturate the latest-wins queue.
    #[must_use]
    pub(crate) fn has_pending_partial_fill_same_or_older_stamp(
        &self,
        stamp: &ProjectionStamp,
    ) -> bool {
        let Ok(pending) = self.pending_requests.lock() else {
            return false;
        };
        pending.iter().any(|entry| {
            entry.reason == PAINT_PARTIAL_FILL_REASON
                && entry.stamp.document == stamp.document
                && entry.stamp.font_state == stamp.font_state
                && entry.stamp.wrap_width_dip == stamp.wrap_width_dip
                && entry.stamp.rope_revision <= stamp.rope_revision
        })
    }

    /// `true` when any pending or in-flight worker request was
    /// submitted with one of `reasons`. P0.8.3 uses this to detect a
    /// `layout_change` / `focus_change` prewarm that justifies the
    /// extended paint-time wait budget. Comparison is `&'static str`
    /// pointer equality first, falling back to equal-bytes when the
    /// caller built its slice from non-static literals (no allocations
    /// in either path).
    #[must_use]
    pub(crate) fn any_pending_with_reasons(&self, reasons: &[&'static str]) -> bool {
        let Ok(pending) = self.pending_requests.lock() else {
            return false;
        };
        pending.iter().any(|entry| reasons.contains(&entry.reason))
    }

    pub(crate) fn record_pending_request_for_probe(&self, seq: u64, stamp: ProjectionStamp) {
        self.record_pending_request_for_probe_with_target_and_reason(
            seq,
            PaneId::fresh(),
            stamp,
            "probe",
        );
    }

    pub(crate) fn record_pending_request_for_probe_with_reason(
        &self,
        seq: u64,
        stamp: ProjectionStamp,
        reason: &'static str,
    ) {
        self.record_pending_request_for_probe_with_target_and_reason(
            seq,
            PaneId::fresh(),
            stamp,
            reason,
        );
    }

    pub(crate) fn record_pending_request_for_probe_with_target_and_reason(
        &self,
        seq: u64,
        target_pane: PaneId,
        stamp: ProjectionStamp,
        reason: &'static str,
    ) {
        self.push_pending_request(PendingProjectionRequest {
            seq,
            target_pane,
            stamp,
            reason,
        });
    }

    fn push_pending_request(&self, request: PendingProjectionRequest) {
        if let Ok(mut pending) = self.pending_requests.lock() {
            pending.push_back(request);
        }
    }

    fn remove_pending_request(&self, seq: u64) {
        if let Ok(mut pending) = self.pending_requests.lock() {
            pending.retain(|request| request.seq != seq);
        }
    }

    /// Take the most recent worker result, if one is available.
    pub(crate) fn take_latest_result(&self) -> Option<ProjectionResult> {
        self.latest_result
            .results
            .lock()
            .ok()
            .and_then(|mut g| g.pop_back())
    }

    /// Take the most recent worker result for `target_pane`, leaving
    /// results for other panes available to their own drains.
    pub(crate) fn take_latest_result_for_target(
        &self,
        target_pane: PaneId,
    ) -> Option<ProjectionResult> {
        self.latest_result
            .results
            .lock()
            .ok()
            .and_then(|mut results| {
                let idx = results
                    .iter()
                    .rposition(|result| result.target_pane == target_pane)?;
                results.remove(idx)
            })
    }

    /// `true` when the worker has published a result the UI has not yet
    /// drained. The shared result queue only holds a result between the
    /// worker's publish and the next paint's `take_latest_result*`, so a
    /// non-empty queue means a paint is owed. The projection-realize
    /// watchdog ([`crate::Window::on_decoration_watchdog_tick`]) uses this
    /// to schedule that paint when the off-thread jump's poll budget
    /// expired before the build landed — otherwise the result sits
    /// unconsumed until unrelated input forces a `WM_PAINT` (the
    /// "focus-into-buffer is blank until I type" regression). The next
    /// paint drains every live pane's result (focused via
    /// `take_latest_result_for_target`, spectators via their own drain,
    /// dead panes via [`Self::retain_results_for_live_panes`]), so a
    /// non-empty queue cannot keep re-triggering the watchdog.
    #[must_use]
    pub(crate) fn has_unconsumed_result(&self) -> bool {
        self.latest_result
            .results
            .lock()
            .ok()
            .is_some_and(|results| !results.is_empty())
    }

    /// Peek the most recent worker result's `font_state` for
    /// `target_pane` without consuming it. Used by the deferred
    /// font-swap path ([`crate::window_font_swap`]) — the swap only
    /// fires when a result built against the pending font_state has
    /// actually landed, so the regular per-paint
    /// [`take_latest_result_for_target`] consumes the same entry on
    /// the very same paint and renders against it.
    ///
    /// Returns `None` when the result queue is empty for the target
    /// or when the mutex is poisoned.
    pub(crate) fn peek_latest_result_font_state_for_target(
        &self,
        target_pane: PaneId,
    ) -> Option<continuity_layout::FontStateId> {
        self.latest_result.results.lock().ok().and_then(|results| {
            let result = results
                .iter()
                .rev()
                .find(|result| result.target_pane == target_pane)?;
            Some(result.stamp.font_state)
        })
    }

    /// Drop queued results for panes that are no longer present in the
    /// live tree. Called by the UI-thread spectator drain before it
    /// routes worker results into the spectator frame cache.
    pub(crate) fn retain_results_for_live_panes(&self, live_panes: &[PaneId]) {
        if let Ok(mut results) = self.latest_result.results.lock() {
            results.retain(|result| live_panes.contains(&result.target_pane));
        }
    }

    /// Block the calling thread for up to `timeout` waiting for the
    /// worker to publish a result. Returns `true` when the result queue
    /// is non-empty on return (already populated or populated during
    /// the wait), `false` on timeout or lock poisoning.
    ///
    /// Paint uses this on its bounded-wait path before falling through
    /// to an inline build — it does *not* consume the result; the
    /// caller's follow-up [`Self::take_latest_result`] does that under
    /// the same mutex, so no notification can be lost between drain
    /// and re-poll.
    #[must_use]
    pub(crate) fn wait_for_result_publication(&self, timeout: Duration) -> bool {
        let Ok(guard) = self.latest_result.results.lock() else {
            return false;
        };
        if !guard.is_empty() {
            return true;
        }
        let Ok((guard, wait_res)) = self.latest_result.publication.wait_timeout(guard, timeout)
        else {
            return false;
        };
        if wait_res.timed_out() {
            !guard.is_empty()
        } else {
            true
        }
    }

    /// Total number of builds the worker has actually performed since
    /// spawn. Submitted-but-coalesced requests do not count.
    pub(crate) fn processed_count(&self) -> u64 {
        self.processed_count.load(Ordering::Relaxed)
    }

    /// Current depth of the bounded command channel. `0` when idle;
    /// `COMMAND_CHANNEL_CAPACITY` when saturated. Used by the periodic
    /// memory-breakdown trace.
    pub(crate) fn queue_depth(&self) -> usize {
        self.cmd_tx.as_ref().map(|tx| tx.len()).unwrap_or(0)
    }

    /// Cmd-channel capacity. Pair with [`Self::queue_depth`] for the
    /// `event:memory_breakdown` `projection_queue_depth` /
    /// `projection_queue_capacity` fields.
    #[must_use]
    pub(crate) fn queue_capacity(&self) -> usize {
        COMMAND_CHANNEL_CAPACITY
    }

    /// Estimated bytes held by the shared wrap cache.
    #[must_use]
    pub(crate) fn wrap_cache_bytes(&self) -> usize {
        self.wrap_cache.byte_size_estimate()
    }

    /// Estimated bytes held by the shared segment cache.
    #[must_use]
    pub(crate) fn segment_cache_bytes(&self) -> usize {
        self.segment_cache.byte_size_estimate()
    }

    /// Test-only: write a synthetic result directly into the result
    /// queue. Bypasses the worker thread so unit tests over the
    /// UI-side acceptance logic do not depend on real DirectWrite or
    /// worker scheduling timing.
    #[cfg(test)]
    pub(crate) fn inject_result_for_test(&self, result: ProjectionResult) {
        if let Ok(mut cell) = self.latest_result.results.lock() {
            push_bounded_result(&mut cell, result);
            self.latest_result.publication.notify_all();
        }
    }
}

pub(super) fn push_bounded_result(
    results: &mut VecDeque<ProjectionResult>,
    result: ProjectionResult,
) {
    results.push_back(result);
    while results.len() > RESULT_QUEUE_CAPACITY {
        let _ = results.pop_front();
    }
}

impl Drop for ProjectionWorker {
    fn drop(&mut self) {
        // Drop the sender first so the worker's recv returns Err.
        drop(self.cmd_tx.take());
        if let Some(thread) = self.thread.take() {
            // The worker exits on its next recv; join with no timeout.
            // Best-effort: ignore a poisoned panic during shutdown.
            let _ = thread.join();
        }
    }
}
