//! Decoration worker pool — N threads consuming `(snapshot, revision)`
//! requests and producing [`Decorations`] results.
//!
//! **Thread ownership**:
//! - The producer side ([`DecoratePool::request`]) is a clone-able sender any
//!   thread may invoke; the latest request per `buffer_id` always wins
//!   (older queued snapshots are silently dropped — only the most recent
//!   revision matters to render).
//! - Each worker thread owns its own `MarkdownParser` (tree-sitter parsers
//!   are not `Send`-shared) and is the sole writer of its in-flight
//!   `Decorations` until it sends.
//! - The consumer side ([`DecoratePool::results`]) is owned by exactly one
//!   UI thread, which compares each result's revision against the current
//!   buffer revision and discards stale snapshots per spec §2.
//!
//! Backpressure: the request queue holds at most one pending request per
//! `buffer_id`. When the producer enqueues a new revision for a buffer
//! that already has work pending, the older request is overwritten in
//! place. This satisfies the §2 rule "hot-path sends … with explicit
//! overflow policy" — overflow policy is _drop-stale, keep-latest_.
//!
//! The pool is shut down when [`DecoratePool`] is dropped: idle workers
//! are woken and joined; a generation that is still blocked is detached
//! so shutdown cannot hang behind the same failure the watchdog is meant
//! to recover from.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use ropey::Rope;

use crate::decorations::Decorations;
use crate::language::Language;
use crate::request_queue::{enqueue_request, is_shutdown, LatestQueue};
use crate::tree_cache::BufferTreeCache;
use crate::worker_watchdog::{WorkerWatchdog, DEFAULT_DECORATE_WORKER_WATCHDOG_TIMEOUT_MS};
use crate::Error;

mod compute;
pub mod parse_trace;
pub mod tree_cache_registry;
use compute::compute_decorations_for_request;
use parse_trace::{DecorationFullParseReason, DecorationParseTrace};
use tree_cache_registry::TreeCacheRegistry;

/// One unit of work for a decoration worker.
///
/// The payload carries the buffer's rope as an `Arc<Rope>` — materializing
/// the rope to a flat `String` is deferred to the worker thread, after
/// the latest-wins queue has collapsed redundant submissions. The
/// producer pays only an `Arc::clone` per submission; the survivor of
/// coalescing pays the one materialization that is actually needed.
#[derive(Clone, Debug)]
pub struct DecorateRequest {
    /// Buffer identifier the request belongs to. UI consumer routes by id.
    pub buffer_id: u128,
    /// Revision the snapshot was taken at.
    pub revision: u64,
    /// Immutable snapshot of the buffer's rope. Cloning the `Arc`
    /// shares the rope across the producer/worker boundary; the
    /// worker materializes a `String` for tree-sitter parse +
    /// markdown extraction only after the latest-wins queue picks it
    /// as the survivor.
    pub rope: Arc<Rope>,
    /// Language detected by the UI producer for this snapshot. Markdown
    /// buffers run the full tree-sitter decoration path; non-Markdown
    /// buffers intentionally produce empty decorations so plain files are
    /// displayed from the canonical rope without markdown projection.
    pub language: Language,
    /// ε.4 — revision the previous successful decoration was computed
    /// against, if any. `None` means the worker should full-reparse
    /// (first request for this buffer, or no prior incremental tree
    /// cached on this worker).
    pub prev_revision: Option<u64>,
    /// ε.4 — rope edit deltas between `prev_revision` and `revision`.
    /// Worker feeds these to `tree.edit(InputEdit { ... })` in stored
    /// order, then calls `parser.parse(source, Some(&old_tree))` for
    /// the incremental reparse. Empty slice ⇒ full reparse expected.
    ///
    /// Use [`empty_deltas`] to construct an empty payload — that
    /// helper hands out a process-wide static `Arc<[]>` clone instead
    /// of allocating a fresh boxed slice per request.
    pub deltas_since_prev: Arc<[crate::RopeEditDeltaWithPoints]>,
    /// Reason to report if this request cannot attempt incremental parse
    /// because no previous revision hint is available.
    pub full_parse_reason: DecorationFullParseReason,
}

/// Process-wide empty `Arc<[RopeEditDeltaWithPoints]>` for callers that have
/// no edits to forward to the worker. Cloning an `Arc` is a single
/// atomic bump; this avoids three independent boxed-slice
/// allocations per decoration submission while the incremental
/// tree-sitter integration is still staged.
#[must_use]
pub fn empty_deltas() -> Arc<[crate::RopeEditDeltaWithPoints]> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<Arc<[crate::RopeEditDeltaWithPoints]>> = OnceLock::new();
    Arc::clone(EMPTY.get_or_init(|| {
        let v: Vec<crate::RopeEditDeltaWithPoints> = Vec::new();
        Arc::from(v.into_boxed_slice())
    }))
}

/// Result emitted by a worker.
#[derive(Debug)]
pub struct DecorateResult {
    /// Owning buffer.
    pub buffer_id: u128,
    /// Outcome — either a fresh decoration snapshot or an error.
    pub outcome: Result<Decorations, Error>,
    /// Parse path taken by the worker.
    pub parse_trace: DecorationParseTrace,
}

/// Watchdog notification emitted after a worker generation is restarted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecorateWorkerRestart {
    /// Worker slot that was restarted.
    pub worker_id: usize,
    /// Buffer whose in-flight request was re-enqueued.
    pub buffer_id: u128,
    /// Revision whose in-flight request was re-enqueued.
    pub revision: u64,
}

/// Returned by [`DecoratePool::request`] when the pool has shut down.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PoolShutdown;

impl std::fmt::Display for PoolShutdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("decoration pool has shut down")
    }
}

impl std::error::Error for PoolShutdown {}

type ComputeDecorations = Arc<
    dyn Fn(
            &DecorateRequest,
            Option<&mut BufferTreeCache>,
        ) -> Option<(Decorations, DecorationParseTrace)>
        + Send
        + Sync
        + 'static,
>;

#[derive(Clone)]
struct WorkerRuntime {
    queue: Arc<(Mutex<LatestQueue>, Condvar)>,
    result_tx: Sender<DecorateResult>,
    watchdog: Arc<WorkerWatchdog>,
    compute: ComputeDecorations,
    use_tree_cache: bool,
    tree_cache_registry: Arc<TreeCacheRegistry>,
}

/// Default worker timeout used by [`DecoratePool::spawn`].
pub const DEFAULT_WORKER_WATCHDOG_TIMEOUT: Duration =
    Duration::from_millis(DEFAULT_DECORATE_WORKER_WATCHDOG_TIMEOUT_MS);

/// Handle to a decoration worker pool.
pub struct DecoratePool {
    /// Shared between the producer ([`DecoratePool::request`], any caller
    /// thread) and the N worker threads. Lock scopes are short — insert,
    /// pop, set-shutdown — so contention is irrelevant.
    queue: Arc<(Mutex<LatestQueue>, Condvar)>,
    result_rx: Receiver<DecorateResult>,
    restart_rx: Receiver<DecorateWorkerRestart>,
    watchdog: Arc<WorkerWatchdog>,
    watchdog_thread: Option<thread::JoinHandle<()>>,
    /// Per-worker `BufferTreeCache` instrumentation registry. Workers
    /// publish their cache state on every cache touch; the UI thread
    /// reads aggregated stats for `event:memory_breakdown` and
    /// `event:buffer_focus_change`.
    tree_cache_registry: Arc<TreeCacheRegistry>,
}

impl DecoratePool {
    /// Spawn `worker_count` decoration workers (clamped to `>= 1`).
    ///
    /// `result_capacity` bounds the result channel — when full, workers
    /// block briefly on send until the UI consumer drains. A reasonable
    /// default is `worker_count * 4`.
    #[must_use]
    pub fn spawn(worker_count: usize, result_capacity: usize) -> Self {
        Self::spawn_with_watchdog_timeout(
            worker_count,
            result_capacity,
            DEFAULT_WORKER_WATCHDOG_TIMEOUT,
        )
    }

    /// Spawn workers with an explicit watchdog timeout.
    #[must_use]
    pub fn spawn_with_watchdog_timeout(
        worker_count: usize,
        result_capacity: usize,
        watchdog_timeout: Duration,
    ) -> Self {
        Self::spawn_with_compute(
            worker_count,
            result_capacity,
            watchdog_timeout,
            Arc::new(compute_decorations_for_request),
            true,
        )
    }

    #[cfg(test)]
    fn spawn_with_compute_for_tests(
        worker_count: usize,
        result_capacity: usize,
        watchdog_timeout: Duration,
        compute: ComputeDecorations,
    ) -> Self {
        Self::spawn_with_compute(
            worker_count,
            result_capacity,
            watchdog_timeout,
            compute,
            false,
        )
    }

    fn spawn_with_compute(
        worker_count: usize,
        result_capacity: usize,
        watchdog_timeout: Duration,
        compute: ComputeDecorations,
        use_tree_cache: bool,
    ) -> Self {
        let workers_n = worker_count.max(1);
        let cap = result_capacity.max(workers_n * 2);
        let queue = Arc::new((Mutex::new(LatestQueue::new(workers_n)), Condvar::new()));
        let (result_tx, result_rx) = bounded::<DecorateResult>(cap);
        let (restart_tx, restart_rx) = bounded::<DecorateWorkerRestart>(workers_n * 4);
        let watchdog = Arc::new(WorkerWatchdog::new(workers_n, watchdog_timeout));
        let tree_cache_registry = Arc::new(TreeCacheRegistry::new(workers_n));
        let worker_runtime = WorkerRuntime {
            queue: Arc::clone(&queue),
            result_tx: result_tx.clone(),
            watchdog: Arc::clone(&watchdog),
            compute: Arc::clone(&compute),
            use_tree_cache,
            tree_cache_registry: Arc::clone(&tree_cache_registry),
        };
        for worker_id in 0..workers_n {
            spawn_worker(
                worker_id,
                watchdog.generation(worker_id),
                worker_runtime.clone(),
            );
        }
        let watchdog_thread = spawn_watchdog_thread(
            Arc::clone(&queue),
            Arc::clone(&watchdog),
            restart_tx,
            result_tx.clone(),
            Arc::clone(&compute),
            use_tree_cache,
            Arc::clone(&tree_cache_registry),
        );
        drop(result_tx);
        Self {
            queue,
            result_rx,
            restart_rx,
            watchdog,
            watchdog_thread: Some(watchdog_thread),
            tree_cache_registry,
        }
    }

    /// Submit a decoration request. Older pending requests for the same
    /// `buffer_id` are silently dropped — only the latest revision per
    /// buffer survives in the queue (drop-stale, keep-latest).
    ///
    /// Returns [`PoolShutdown`] only if the pool has shut down (its
    /// `Drop` ran or a worker poisoned the queue lock).
    pub fn request(&self, req: DecorateRequest) -> Result<(), PoolShutdown> {
        enqueue_request(&self.queue, req)
    }

    /// The receiver UI threads pump for fresh `Decorations`.
    #[must_use]
    pub fn results(&self) -> &Receiver<DecorateResult> {
        &self.result_rx
    }

    /// Watchdog notifications for UI feedback.
    #[must_use]
    pub fn worker_restarts(&self) -> &Receiver<DecorateWorkerRestart> {
        &self.restart_rx
    }

    /// Number of workers in the pool.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.watchdog.len()
    }

    /// Update the non-responsive-worker timeout.
    pub fn set_watchdog_timeout(&self, timeout: Duration) {
        self.watchdog.set_timeout(timeout);
    }

    /// Number of pending requests across all worker slots (for tests
    /// / introspection).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.0.lock().map(|g| g.pending()).unwrap_or(0)
    }

    /// Aggregate byte-size estimate across every worker's
    /// [`BufferTreeCache`]. Sums per-worker
    /// [`crate::BufferTreeCache::byte_size_estimate`] — see that method
    /// for the lower-bound caveat. When more than one worker caches the
    /// same buffer the figure intentionally double-counts, which is the
    /// signal Block 0.1 of the memory optimization plan asked for.
    #[must_use]
    pub fn tree_cache_bytes_estimate(&self) -> usize {
        self.tree_cache_registry.aggregate_bytes()
    }

    /// `true` iff at least one worker has a cached parse tree for
    /// `buffer_id`. Used by `event:buffer_focus_change` to predict
    /// whether the next decoration request for a freshly-focused buffer
    /// will hit a worker's tree cache.
    #[must_use]
    pub fn has_cached_tree(&self, buffer_id: u128) -> bool {
        self.tree_cache_registry.any_worker_has_buffer(buffer_id)
    }

    /// ε.4 — broadcast a "drop this buffer's cached tree" signal to
    /// every worker. Call when a buffer is closed so per-worker
    /// `BufferTreeCache` entries don't accumulate forever in long
    /// sessions. Workers drain the signal lazily on their next
    /// request poll; if a worker happens to be sleeping on the
    /// condvar, the cleanup waits until traffic resumes for one of
    /// its routed buffers (acceptable — trees are cheap and the
    /// bounded LRU is the upper bound).
    pub fn drop_buffer(&self, buffer_id: u128) -> Result<(), PoolShutdown> {
        let (lock, _cvar) = &*self.queue;
        let mut guard = lock.lock().map_err(|_| PoolShutdown)?;
        if guard.is_shutdown() {
            return Err(PoolShutdown);
        }
        guard.broadcast_drop_buffer(buffer_id);
        Ok(())
    }
}

impl Drop for DecoratePool {
    fn drop(&mut self) {
        if let Ok(mut g) = self.queue.0.lock() {
            g.shutdown_and_clear();
        }
        self.queue.1.notify_all();
        if let Some(handle) = self.watchdog_thread.take() {
            let _ = handle.join();
        }
        for handle in self.watchdog.take_idle_handles() {
            let _ = handle.join();
        }
    }
}

fn spawn_worker(worker_id: usize, generation: u64, runtime: WorkerRuntime) {
    let watchdog = Arc::clone(&runtime.watchdog);
    let handle = thread::Builder::new()
        .name(format!("decorate-worker-{worker_id}"))
        .spawn(move || worker_loop(worker_id, generation, runtime))
        .expect("invariant: spawning a decoration worker should not fail");
    watchdog.install_handle(worker_id, generation, handle);
}

fn spawn_watchdog_thread(
    queue: Arc<(Mutex<LatestQueue>, Condvar)>,
    watchdog: Arc<WorkerWatchdog>,
    restart_tx: Sender<DecorateWorkerRestart>,
    result_tx: Sender<DecorateResult>,
    compute: ComputeDecorations,
    use_tree_cache: bool,
    tree_cache_registry: Arc<TreeCacheRegistry>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("decorate-watchdog".into())
        .spawn(move || {
            watchdog_loop(
                queue,
                watchdog,
                restart_tx,
                result_tx,
                compute,
                use_tree_cache,
                tree_cache_registry,
            )
        })
        .expect("invariant: spawning decoration watchdog should not fail")
}

fn watchdog_loop(
    queue: Arc<(Mutex<LatestQueue>, Condvar)>,
    watchdog: Arc<WorkerWatchdog>,
    restart_tx: Sender<DecorateWorkerRestart>,
    result_tx: Sender<DecorateResult>,
    compute: ComputeDecorations,
    use_tree_cache: bool,
    tree_cache_registry: Arc<TreeCacheRegistry>,
) {
    while !is_shutdown(&queue) {
        let timeout = watchdog.timeout();
        let poll = (timeout / 4).clamp(Duration::from_millis(10), Duration::from_millis(250));
        std::thread::sleep(poll);
        if is_shutdown(&queue) {
            return;
        }
        for plan in watchdog.timed_out_workers() {
            let event = DecorateWorkerRestart {
                worker_id: plan.worker_id,
                buffer_id: plan.request.buffer_id,
                revision: plan.request.revision,
            };
            if enqueue_request(&queue, plan.request.clone()).is_err() {
                return;
            }
            spawn_worker(
                plan.worker_id,
                plan.generation,
                WorkerRuntime {
                    queue: Arc::clone(&queue),
                    result_tx: result_tx.clone(),
                    watchdog: Arc::clone(&watchdog),
                    compute: Arc::clone(&compute),
                    use_tree_cache,
                    tree_cache_registry: Arc::clone(&tree_cache_registry),
                },
            );
            match restart_tx.try_send(event) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

fn worker_loop(worker_id: usize, generation: u64, runtime: WorkerRuntime) {
    let mut tree_cache = BufferTreeCache::new();
    let (lock, cvar) = &*runtime.queue;
    loop {
        if !runtime
            .watchdog
            .is_current_generation(worker_id, generation)
        {
            return;
        }
        let (req, dropped_buffer_ids) = {
            let mut guard = match lock.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            loop {
                if guard.is_shutdown() {
                    return;
                }
                if let Some(req) = guard.pop_one_for_worker(worker_id) {
                    // Drain pending drops while we hold the lock so
                    // the cache update happens atomically with the
                    // request the worker is about to process.
                    let drops = guard.take_drops_for_worker(worker_id);
                    break (req, drops);
                }
                guard = match cvar.wait(guard) {
                    Ok(g) => g,
                    Err(_) => return,
                };
            }
        };
        // ε.4 — apply drop signals before processing the next
        // request so closed buffers cannot keep stale parse trees
        // alive until LRU pressure happens to evict them.
        let mut cache_touched = false;
        for id in &dropped_buffer_ids {
            tree_cache.drop_buffer(*id);
            cache_touched = true;
        }
        runtime
            .watchdog
            .start_work(worker_id, generation, req.clone());
        let computed_result = if runtime.use_tree_cache {
            cache_touched = true;
            panic::catch_unwind(AssertUnwindSafe(|| {
                (runtime.compute)(&req, Some(&mut tree_cache))
            }))
        } else {
            panic::catch_unwind(AssertUnwindSafe(|| (runtime.compute)(&req, None)))
        };
        let mut panic_message = None;
        let computed = match computed_result {
            Ok(computed) => computed,
            Err(payload) => {
                let message = panic_payload_to_string(&payload);
                eprintln!(
                    "continuity-decorate: worker {worker_id} recovered from panic on buffer {} revision {}: {message}",
                    req.buffer_id, req.revision
                );
                tree_cache = BufferTreeCache::new();
                cache_touched = true;
                panic_message = Some(message);
                if let Some(slot) = runtime.tree_cache_registry.slot(worker_id) {
                    slot.publish(&tree_cache);
                }
                None
            }
        };
        if cache_touched {
            if let Some(slot) = runtime.tree_cache_registry.slot(worker_id) {
                slot.publish(&tree_cache);
            }
        }
        let (outcome, parse_trace) = match computed {
            Some((decorations, parse_trace)) => (Ok(decorations), parse_trace),
            None => {
                let outcome = if let Some(message) = panic_message {
                    Err(Error::WorkerPanic(message))
                } else {
                    Err(Error::LanguageLoad(
                        "tree-sitter-md grammar failed to load on decoration worker".to_string(),
                    ))
                };
                (
                    outcome,
                    DecorationParseTrace::Full {
                        reason: DecorationFullParseReason::NoPrevTree,
                        elapsed_us: 0,
                        tree_query_us: 0,
                        decoration_compute_us: 0,
                    },
                )
            }
        };
        if !runtime.watchdog.finish_work(worker_id, generation) {
            return;
        }
        if runtime
            .result_tx
            .send(DecorateResult {
                buffer_id: req.buffer_id,
                outcome,
                parse_trace,
            })
            .is_err()
        {
            return;
        }
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests;
