//! Optional synchronous ephemeral host composition.

use std::thread::ThreadId;

use continuity_buffer::{BufferId, Revision};
use continuity_engine::{Engine, EngineSnapshot};

use crate::{
    apply_editor_operation, EditorIntent, EditorOperation, Error, HostEvent, HostEventBatch,
    Invalidation, OperationRequest,
};

/// Synchronous ephemeral host around [`continuity_engine::Engine`].
///
/// **Thread ownership:** the thread that constructs the runtime owns all
/// mutable state. Calls from another thread return [`Error::WrongThread`].
/// Dispatch never invokes callbacks; it returns a complete event batch after
/// releasing every mutable engine borrow.
pub struct HostRuntime {
    engine: continuity_engine::Engine,
    owner_thread: ThreadId,
    next_event_sequence: u64,
    is_closed: bool,
}

impl HostRuntime {
    /// Construct an empty ephemeral runtime on the current thread.
    #[must_use]
    pub fn new() -> Self {
        Self::from_engine(Engine::new())
    }

    /// Wrap a caller-prepared storage-neutral engine on the current thread.
    ///
    /// The runtime takes exclusive ownership of the engine. It does not load,
    /// save, or otherwise contact a persistence implementation.
    #[must_use]
    pub fn from_engine(engine: Engine) -> Self {
        Self {
            engine,
            owner_thread: std::thread::current().id(),
            next_event_sequence: 1,
            is_closed: false,
        }
    }

    /// Open a buffer without files or a database.
    ///
    /// # Errors
    ///
    /// Returns an affinity or lifecycle error when called after moving the
    /// runtime to another thread or after teardown.
    pub fn open_buffer(&mut self, text: &str) -> Result<BufferId, Error> {
        self.validate_call()?;
        let id = self.engine.open_buffer(text);
        self.engine.drain_events();
        Ok(id)
    }

    /// Load host-owned text under an existing buffer id and revision.
    ///
    /// # Errors
    ///
    /// Returns an affinity or lifecycle error after teardown or when called
    /// from a thread other than the runtime owner.
    pub fn load_buffer(
        &mut self,
        buffer_id: BufferId,
        text: &str,
        revision: Revision,
    ) -> Result<BufferId, Error> {
        self.validate_call()?;
        let id = self.engine.load_buffer(buffer_id, text, revision);
        self.engine.drain_events();
        Ok(id)
    }

    /// Capture immutable text, selections, revision, and read-only state.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownBuffer`] when the id is not open.
    pub fn snapshot(&self, buffer_id: BufferId) -> Result<EngineSnapshot, Error> {
        self.engine.snapshot(buffer_id).ok_or(Error::UnknownBuffer)
    }

    /// Current revision for one buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownBuffer`] when the id is not open.
    pub fn revision(&self, buffer_id: BufferId) -> Result<Revision, Error> {
        self.engine.revision(buffer_id).ok_or(Error::UnknownBuffer)
    }

    /// Copy current text for one buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownBuffer`] when the id is not open.
    pub fn text(&self, buffer_id: BufferId) -> Result<String, Error> {
        self.engine.text(buffer_id).ok_or(Error::UnknownBuffer)
    }

    /// Dispatch one normalized intent and return its complete ordered events.
    ///
    /// The returned value is the callback boundary: bindings must deliver it
    /// only after this method returns. A host may synchronously dispatch a new
    /// intent while delivering the prior batch because the engine is no
    /// longer borrowed.
    ///
    /// # Errors
    ///
    /// Returns affinity, lifecycle, stale-revision, or engine errors.
    pub fn dispatch(&mut self, intent: EditorIntent) -> Result<HostEventBatch, Error> {
        self.validate_call()?;
        let mut events = Vec::new();
        match intent {
            EditorIntent::Operation(request) => {
                let result = apply_editor_operation(&mut self.engine, &request)?;
                let has_change = result.change.is_some();
                if let Some(change) = result.change {
                    events.push(HostEvent::Change(Box::new(change)));
                    events.push(HostEvent::Invalidate(Invalidation::Content));
                }
                if result.selections_changed {
                    events.push(HostEvent::Invalidate(Invalidation::Selection));
                }
                if has_change || result.selections_changed {
                    self.push_selection_event(request.buffer_id, result.revision, &mut events)?;
                }
                self.engine.drain_events();
            }
            EditorIntent::Select(selection) => {
                let revision = self
                    .engine
                    .revision(selection.buffer_id)
                    .ok_or(Error::UnknownBuffer)?;
                let request = OperationRequest {
                    buffer_id: selection.buffer_id,
                    expected_revision: Some(revision),
                    timestamp_ms: 0,
                    operation: EditorOperation::SetSelections(selection.selections),
                };
                let result = apply_editor_operation(&mut self.engine, &request)?;
                events.push(HostEvent::Invalidate(Invalidation::Selection));
                self.push_selection_event(selection.buffer_id, result.revision, &mut events)?;
                self.engine.drain_events();
            }
            EditorIntent::Navigate(navigation) => {
                events.push(HostEvent::NavigationRequested(navigation));
            }
            EditorIntent::DispatchCommand { name, target } => {
                events.push(HostEvent::CommandRequested { name, target });
            }
            EditorIntent::ViewportChanged(viewport) => {
                events.push(HostEvent::ViewportChanged(viewport));
                events.push(HostEvent::Invalidate(Invalidation::Viewport));
            }
            EditorIntent::Scroll(_) => {
                events.push(HostEvent::Invalidate(Invalidation::Viewport));
            }
            EditorIntent::Focus(focus) => {
                events.push(HostEvent::FocusChanged(focus));
                events.push(HostEvent::Invalidate(Invalidation::InputState));
            }
            EditorIntent::Pointer(pointer) => events.push(HostEvent::Pointer(pointer)),
            EditorIntent::Composition(composition) => {
                events.push(HostEvent::Composition(composition));
                events.push(HostEvent::Invalidate(Invalidation::InputState));
            }
            EditorIntent::Request(request) => events.push(HostEvent::HostRequest(request)),
        }
        Ok(self.finish_batch(events))
    }

    /// Tear down this runtime. Repeated teardown is harmless; later dispatch
    /// calls return [`Error::Closed`]. No persistence or host callback occurs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WrongThread`] when called away from the owner thread.
    pub fn close(&mut self) -> Result<(), Error> {
        if std::thread::current().id() != self.owner_thread {
            return Err(Error::WrongThread);
        }
        self.is_closed = true;
        Ok(())
    }

    fn validate_call(&self) -> Result<(), Error> {
        if std::thread::current().id() != self.owner_thread {
            return Err(Error::WrongThread);
        }
        if self.is_closed {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn finish_batch(&mut self, events: Vec<HostEvent>) -> HostEventBatch {
        let sequence = self.next_event_sequence;
        self.next_event_sequence = self.next_event_sequence.saturating_add(1);
        HostEventBatch { sequence, events }
    }

    fn push_selection_event(
        &self,
        buffer_id: BufferId,
        revision: Revision,
        events: &mut Vec<HostEvent>,
    ) -> Result<(), Error> {
        let selections = self
            .engine
            .selections(buffer_id)
            .ok_or(Error::UnknownBuffer)?
            .to_vec();
        events.push(HostEvent::SelectionChanged {
            buffer_id,
            revision,
            selections,
        });
        Ok(())
    }
}

impl Default for HostRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Revision;
    use continuity_engine::Engine;

    use super::HostRuntime;

    #[test]
    fn prepared_engine_snapshot_is_preserved_without_io() {
        let mut engine = Engine::new();
        let buffer_id = engine.open_buffer("prepared");
        let runtime = HostRuntime::from_engine(engine);

        let snapshot = runtime
            .snapshot(buffer_id)
            .expect("buffer should remain open");
        assert_eq!(snapshot.rope.rope().to_string(), "prepared");
        assert_eq!(snapshot.rope.revision(), Revision::INITIAL);
    }

    #[test]
    fn host_revision_load_is_visible_in_snapshot() {
        let mut runtime = HostRuntime::new();
        let buffer_id = continuity_buffer::BufferId::new();
        runtime
            .load_buffer(buffer_id, "loaded", Revision(42))
            .expect("host load should succeed");

        let snapshot = runtime
            .snapshot(buffer_id)
            .expect("buffer should be loaded");
        assert_eq!(snapshot.rope.rope().to_string(), "loaded");
        assert_eq!(snapshot.rope.revision(), Revision(42));
    }
}
