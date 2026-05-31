//! [`EditorHandle`]: a thread-safe client for the editor core thread.
//!
//! Spawning consumes a [`PersistClient`] (the core thread sends every edit
//! and policy-driven snapshot through it) and an `Arc<dyn Clock>` (used to
//! timestamp persisted rows and drive the snapshot policy). The core thread
//! is the single owner of `EditorState`, the per-buffer
//! [`UndoOrchestrator`] state, and the snapshot trackers.
//!
//! The public method surface is grouped by topic into sibling modules and
//! attached via `impl EditorHandle` blocks:
//!
//! - [`buffers`] — buffer lifecycle and inspection
//! - [`edits`] — edit and selection mutation
//! - [`undo`] — undo / redo / tree picker
//! - [`streams`] — event subscription, snapshot policy, rope deltas,
//!   pending snapshot labels
//!
//! The core-thread message loop itself lives in [`core_loop`].

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use ahash::AHashMap;
use continuity_buffer::BufferId;
use continuity_persist::PersistClient;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

use crate::clock::Clock;
use crate::message::{EditEvent, EditorMessage};
use crate::policy::{SnapshotPolicy, SnapshotTracker};
use crate::undo::UndoOrchestrator;
use crate::EditorState;

mod buffers;
mod core_loop;
mod edits;
mod streams;
mod undo;

use self::core_loop::core_loop;

/// Handle to a running editor core thread.
///
/// The handle owns the cmd-side `Sender` and the core thread's `JoinHandle`.
/// Drop (or call [`Self::shutdown`]) to flush a final snapshot for every
/// dirty buffer and stop the thread cleanly.
pub struct EditorHandle {
    cmd_tx: Sender<EditorMessage>,
    event_rx: Receiver<EditEvent>,
    join: Option<JoinHandle<()>>,
}

impl EditorHandle {
    /// Spawn the editor core thread and return a handle to it.
    pub fn spawn(persist: PersistClient, clock: Arc<dyn Clock>) -> Self {
        Self::spawn_with_policy(persist, clock, SnapshotPolicy::default())
    }

    /// Spawn with a custom snapshot policy.
    pub fn spawn_with_policy(
        persist: PersistClient,
        clock: Arc<dyn Clock>,
        policy: SnapshotPolicy,
    ) -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<EditorMessage>();
        let (event_tx, event_rx) = unbounded::<EditEvent>();
        let join = thread::Builder::new()
            .name("continuity-core".into())
            .spawn(move || {
                let mut state = EditorState::new();
                let mut trackers: AHashMap<BufferId, SnapshotTracker> = AHashMap::new();
                let mut pending_labels: AHashMap<BufferId, String> = AHashMap::new();
                let mut delta_history: crate::dispatch::DeltaHistory = AHashMap::new();
                let mut undo = UndoOrchestrator::new();
                core_loop(
                    &mut state,
                    &mut trackers,
                    &mut pending_labels,
                    &mut delta_history,
                    &mut undo,
                    &cmd_rx,
                    &event_tx,
                    &persist,
                    clock.as_ref(),
                    policy,
                );
                let _ = event_tx.send(EditEvent::Shutdown);
            })
            .expect("spawn continuity-core thread");
        Self {
            cmd_tx,
            event_rx,
            join: Some(join),
        }
    }

    /// Send a one-shot request to the core thread and block on its
    /// reply. Every public method that needs a synchronous response
    /// from the core thread funnels through this helper.
    fn round_trip<R: Send>(&self, build: impl FnOnce(Sender<R>) -> EditorMessage) -> R {
        let (tx, rx) = bounded(1);
        self.cmd_tx.send(build(tx)).expect("core thread alive");
        rx.recv().expect("reply")
    }

    /// Stop the core thread and wait for it to exit. Idempotent.
    pub fn shutdown(mut self) {
        let _ = self.cmd_tx.send(EditorMessage::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for EditorHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(EditorMessage::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
