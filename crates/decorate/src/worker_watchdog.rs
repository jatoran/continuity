//! Per-worker progress tracking for the decoration pool watchdog.
//!
//! The watchdog state is owned by the decoration pool. Worker threads
//! update their own slot when they start and finish a request; the
//! watchdog thread scans those slots and rotates any worker generation
//! that has been working longer than the configured timeout.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::pool::DecorateRequest;

/// Default non-responsive-worker timeout, in milliseconds.
pub const DEFAULT_DECORATE_WORKER_WATCHDOG_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug)]
enum WorkerActivity {
    Idle {
        _last_progress_at: Instant,
    },
    Working {
        last_progress_at: Instant,
        request: DecorateRequest,
    },
}

#[derive(Debug)]
struct WorkerSlot {
    generation: u64,
    activity: WorkerActivity,
    handle: Option<JoinHandle<()>>,
}

/// Returned when the watchdog rotates a timed-out worker generation.
#[derive(Debug)]
pub(crate) struct WorkerRestartPlan {
    /// Worker slot that timed out.
    pub worker_id: usize,
    /// New generation that should be spawned for the same slot.
    pub generation: u64,
    /// Request the timed-out generation was processing.
    pub request: DecorateRequest,
    old_handle: Option<JoinHandle<()>>,
}

impl Drop for WorkerRestartPlan {
    fn drop(&mut self) {
        // Dropping the handle detaches the old generation. Rust cannot
        // force-stop a blocked OS thread; generation checks prevent any
        // late result from being accepted if it eventually returns.
        let _ = self.old_handle.take();
    }
}

/// Shared progress observer for all worker slots.
#[derive(Debug)]
pub(crate) struct WorkerWatchdog {
    timeout: std::sync::atomic::AtomicU64,
    slots: std::sync::Mutex<Vec<WorkerSlot>>,
}

impl WorkerWatchdog {
    /// Build one slot per worker.
    #[must_use]
    pub(crate) fn new(worker_count: usize, timeout: Duration) -> Self {
        let now = Instant::now();
        let mut slots = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            slots.push(WorkerSlot {
                generation: 0,
                activity: WorkerActivity::Idle {
                    _last_progress_at: now,
                },
                handle: None,
            });
        }
        Self {
            timeout: std::sync::atomic::AtomicU64::new(timeout.as_millis() as u64),
            slots: std::sync::Mutex::new(slots),
        }
    }

    /// Current timeout.
    #[must_use]
    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_millis(
            self.timeout
                .load(std::sync::atomic::Ordering::Relaxed)
                .max(1),
        )
    }

    /// Update the timeout used by future watchdog scans.
    pub(crate) fn set_timeout(&self, timeout: Duration) {
        self.timeout.store(
            timeout.as_millis().max(1) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Number of worker slots.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.slots.lock().map(|slots| slots.len()).unwrap_or(0)
    }

    /// Current generation for `worker_id`.
    #[must_use]
    pub(crate) fn generation(&self, worker_id: usize) -> u64 {
        self.slots
            .lock()
            .ok()
            .and_then(|slots| slots.get(worker_id).map(|slot| slot.generation))
            .unwrap_or(0)
    }

    /// Install a worker thread handle for a slot generation.
    pub(crate) fn install_handle(&self, worker_id: usize, generation: u64, handle: JoinHandle<()>) {
        if let Ok(mut slots) = self.slots.lock() {
            if let Some(slot) = slots.get_mut(worker_id) {
                if slot.generation == generation {
                    slot.handle = Some(handle);
                }
            }
        }
    }

    /// Mark a worker generation as actively processing `request`.
    pub(crate) fn start_work(&self, worker_id: usize, generation: u64, request: DecorateRequest) {
        if let Ok(mut slots) = self.slots.lock() {
            let Some(slot) = slots.get_mut(worker_id) else {
                return;
            };
            if slot.generation != generation {
                return;
            }
            slot.activity = WorkerActivity::Working {
                last_progress_at: Instant::now(),
                request,
            };
        }
    }

    /// Mark a worker generation idle. Returns `false` if that generation
    /// has already been replaced and its result must be ignored.
    #[must_use]
    pub(crate) fn finish_work(&self, worker_id: usize, generation: u64) -> bool {
        let now = Instant::now();
        let Ok(mut slots) = self.slots.lock() else {
            return false;
        };
        let Some(slot) = slots.get_mut(worker_id) else {
            return false;
        };
        if slot.generation != generation {
            return false;
        }
        slot.activity = WorkerActivity::Idle {
            _last_progress_at: now,
        };
        true
    }

    /// Return `false` when the generation has been retired.
    #[must_use]
    pub(crate) fn is_current_generation(&self, worker_id: usize, generation: u64) -> bool {
        self.slots
            .lock()
            .ok()
            .and_then(|slots| {
                slots
                    .get(worker_id)
                    .map(|slot| slot.generation == generation)
            })
            .unwrap_or(false)
    }

    /// Rotate every worker generation that exceeded the timeout.
    #[must_use]
    pub(crate) fn timed_out_workers(&self) -> Vec<WorkerRestartPlan> {
        let timeout = self.timeout();
        let now = Instant::now();
        let Ok(mut slots) = self.slots.lock() else {
            return Vec::new();
        };
        let mut plans = Vec::new();
        for (worker_id, slot) in slots.iter_mut().enumerate() {
            let WorkerActivity::Working {
                last_progress_at,
                request,
            } = &slot.activity
            else {
                continue;
            };
            if now.duration_since(*last_progress_at) < timeout {
                continue;
            }
            let request = request.clone();
            let old_handle = slot.handle.take();
            slot.generation = slot.generation.saturating_add(1);
            let generation = slot.generation;
            slot.activity = WorkerActivity::Idle {
                _last_progress_at: now,
            };
            plans.push(WorkerRestartPlan {
                worker_id,
                generation,
                request,
                old_handle,
            });
        }
        plans
    }

    /// Take idle worker handles for clean shutdown. Active generations are
    /// detached so a blocked worker cannot hang application shutdown.
    #[must_use]
    pub(crate) fn take_idle_handles(&self) -> Vec<JoinHandle<()>> {
        let Ok(mut slots) = self.slots.lock() else {
            return Vec::new();
        };
        let mut handles = Vec::new();
        for slot in &mut *slots {
            if matches!(slot.activity, WorkerActivity::Idle { .. }) {
                if let Some(handle) = slot.handle.take() {
                    handles.push(handle);
                }
            }
        }
        handles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn request(revision: u64) -> DecorateRequest {
        DecorateRequest {
            buffer_id: 7,
            revision,
            rope: Arc::new(ropey::Rope::from_str("body")),
            language: crate::Language::Markdown,
            prev_revision: None,
            deltas_since_prev: crate::empty_deltas(),
            full_parse_reason: crate::pool::parse_trace::DecorationFullParseReason::NoPrevTree,
        }
    }

    #[test]
    fn observer_rotates_timed_out_generation() {
        let watchdog = WorkerWatchdog::new(1, Duration::from_millis(1));
        watchdog.start_work(0, 0, request(3));
        std::thread::sleep(Duration::from_millis(5));

        let plans = watchdog.timed_out_workers();

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].worker_id, 0);
        assert_eq!(plans[0].generation, 1);
        assert_eq!(plans[0].request.revision, 3);
        assert!(!watchdog.is_current_generation(0, 0));
        assert!(watchdog.is_current_generation(0, 1));
    }

    #[test]
    fn finish_rejects_retired_generation() {
        let watchdog = WorkerWatchdog::new(1, Duration::from_millis(1));
        watchdog.start_work(0, 0, request(3));
        std::thread::sleep(Duration::from_millis(5));
        let _plans = watchdog.timed_out_workers();

        assert!(!watchdog.finish_work(0, 0));
        assert!(watchdog.finish_work(0, 1));
    }
}
