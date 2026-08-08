#![warn(missing_docs)]
//! Python facade for the synchronous, storage-neutral Continuity engine.

use std::thread::ThreadId;

use continuity_buffer::{BufferId, Revision};
use continuity_engine::{Engine, SelectionEdit};
use continuity_text::{Position, Selection};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::{
    pyclass, pymethods, pymodule, Bound, Py, PyAny, PyErr, PyModule, PyRefMut, PyResult, Python,
};
use pyo3::types::PyModuleMethods;

/// Immutable Python snapshot returned by [`Editor::snapshot`].
#[pyclass(frozen, module = "continuity_editor._continuity_editor")]
pub struct Snapshot {
    text: String,
    revision: u64,
    carets: Vec<(u32, u32)>,
}

#[pymethods]
impl Snapshot {
    /// Canonical UTF-8 document text.
    #[getter]
    fn text(&self) -> &str {
        &self.text
    }

    /// Current document revision.
    #[getter]
    fn revision(&self) -> u64 {
        self.revision
    }

    /// Current caret heads as `(line, utf8_byte_in_line)` pairs.
    #[getter]
    fn carets(&self) -> Vec<(u32, u32)> {
        self.carets.clone()
    }
}

/// One headless editor document owned by its constructing Python thread.
#[pyclass(module = "continuity_editor._continuity_editor")]
pub struct Editor {
    engine: Engine,
    buffer_id: BufferId,
    owner_thread: ThreadId,
    callback: Option<Py<PyAny>>,
    is_in_callback: bool,
    is_closed: bool,
}

// SAFETY: Python may move the wrapper between interpreter threads, but every
// method checks `owner_thread` before touching the non-Send engine. Engine
// mutation and callback state remain confined to the construction thread.
unsafe impl Send for Editor {}
// SAFETY: shared Python access performs the immutable owner-thread check first;
// only the owner may continue to read engine or lifecycle state. PyO3's borrow
// guard serializes mutable method access before the Rust method body runs.
unsafe impl Sync for Editor {}

#[pymethods]
impl Editor {
    /// Construct an ephemeral editor from host-owned text and revision.
    #[new]
    #[pyo3(signature = (text = "", revision = 0))]
    fn new(text: &str, revision: u64) -> Self {
        let mut engine = Engine::new();
        let buffer_id = BufferId::new();
        engine.load_buffer(buffer_id, text, Revision(revision));
        engine.drain_events();
        Self {
            engine,
            buffer_id,
            owner_thread: std::thread::current().id(),
            callback: None,
            is_in_callback: false,
            is_closed: false,
        }
    }

    /// Replace all selections with UTF-8 source carets.
    fn set_carets(&mut self, carets: Vec<(u32, u32)>) -> PyResult<()> {
        self.validate_call()?;
        let selections = carets
            .into_iter()
            .map(|(line, byte_in_line)| Selection::caret_at(Position::new(line, byte_in_line)))
            .collect();
        self.engine
            .set_selections(self.buffer_id, selections)
            .map_err(engine_error)
    }

    /// Insert text at every selection and return the resulting revision.
    #[pyo3(signature = (text, timestamp_ms = 0))]
    fn insert_text(&mut self, py: Python<'_>, text: String, timestamp_ms: i64) -> PyResult<u64> {
        self.validate_call()?;
        let revision = self
            .engine
            .apply_selection_edit(
                self.buffer_id,
                &SelectionEdit::InsertText(text),
                timestamp_ms,
            )
            .map_err(engine_error)?
            .map(|batch| batch.revision_after.get());
        self.notify_change(py, revision)?;
        self.current_revision()
    }

    /// Delete backward at every selection and return the current revision.
    #[pyo3(signature = (timestamp_ms = 0))]
    fn delete_backward(&mut self, py: Python<'_>, timestamp_ms: i64) -> PyResult<u64> {
        self.validate_call()?;
        let revision = self
            .engine
            .apply_selection_edit(self.buffer_id, &SelectionEdit::DeleteBack, timestamp_ms)
            .map_err(engine_error)?
            .map(|batch| batch.revision_after.get());
        self.notify_change(py, revision)?;
        self.current_revision()
    }

    /// Undo the current edit group and return the current revision.
    #[pyo3(signature = (timestamp_ms = 0))]
    fn undo(&mut self, py: Python<'_>, timestamp_ms: i64) -> PyResult<u64> {
        self.validate_call()?;
        let revision = self
            .engine
            .undo(self.buffer_id, timestamp_ms)
            .map_err(engine_error)?
            .map(|batch| batch.revision_after.get());
        self.notify_change(py, revision)?;
        self.current_revision()
    }

    /// Redo the preferred edit branch and return the current revision.
    #[pyo3(signature = (timestamp_ms = 0))]
    fn redo(&mut self, py: Python<'_>, timestamp_ms: i64) -> PyResult<u64> {
        self.apply_redo(py, timestamp_ms, false)
    }

    /// Redo an alternate edit branch and return the current revision.
    #[pyo3(signature = (timestamp_ms = 0))]
    fn redo_alternate(&mut self, py: Python<'_>, timestamp_ms: i64) -> PyResult<u64> {
        self.apply_redo(py, timestamp_ms, true)
    }

    /// Capture text, revision, and carets atomically from the engine.
    fn snapshot(&self) -> PyResult<Snapshot> {
        self.validate_call()?;
        let snapshot = self
            .engine
            .snapshot(self.buffer_id)
            .ok_or_else(|| PyRuntimeError::new_err("editor buffer is unavailable"))?;
        Ok(Snapshot {
            text: snapshot.rope.rope().to_string(),
            revision: snapshot.rope.revision().get(),
            carets: snapshot
                .selections
                .iter()
                .map(|selection| (selection.head.line, selection.head.byte_in_line))
                .collect(),
        })
    }

    /// Return byte deltas newer than a revision as `(at, removed, inserted)` tuples.
    fn deltas_since(&self, since_revision: u64) -> PyResult<Vec<(usize, usize, usize)>> {
        self.validate_call()?;
        Ok(self
            .engine
            .deltas_since(self.buffer_id, since_revision)
            .0
            .into_iter()
            .map(|delta| (delta.at, delta.removed_bytes, delta.inserted_bytes))
            .collect())
    }

    /// Register a callable receiving the revision after each real mutation.
    #[pyo3(signature = (callback = None))]
    fn set_change_callback(&mut self, callback: Option<Py<PyAny>>) -> PyResult<()> {
        self.validate_call()?;
        self.callback = callback;
        Ok(())
    }

    /// Explicitly tear down the editor and release its callback.
    fn close(&mut self) -> PyResult<()> {
        self.validate_thread()?;
        if self.is_in_callback {
            return Err(PyRuntimeError::new_err(
                "editor calls are rejected inside its change callback",
            ));
        }
        self.callback = None;
        self.is_closed = true;
        Ok(())
    }

    /// Enter a context-manager scope.
    fn __enter__(slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        slf
    }

    /// Leave a context-manager scope with explicit teardown.
    fn __exit__(
        &mut self,
        _exception_type: Option<&Bound<'_, PyAny>>,
        _exception: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.close()?;
        Ok(false)
    }
}

impl Editor {
    fn apply_redo(
        &mut self,
        py: Python<'_>,
        timestamp_ms: i64,
        is_alternate: bool,
    ) -> PyResult<u64> {
        self.validate_call()?;
        let result = if is_alternate {
            self.engine.redo_alternate(self.buffer_id, timestamp_ms)
        } else {
            self.engine.redo(self.buffer_id, timestamp_ms)
        };
        let revision = result
            .map_err(engine_error)?
            .map(|batch| batch.revision_after.get());
        self.notify_change(py, revision)?;
        self.current_revision()
    }

    fn notify_change(&mut self, py: Python<'_>, revision: Option<u64>) -> PyResult<()> {
        let (Some(callback), Some(revision)) = (&self.callback, revision) else {
            return Ok(());
        };
        self.is_in_callback = true;
        let result = callback.call1(py, (revision,)).map(|_| ());
        self.is_in_callback = false;
        result
    }

    fn current_revision(&self) -> PyResult<u64> {
        self.engine
            .revision(self.buffer_id)
            .map(Revision::get)
            .ok_or_else(|| PyRuntimeError::new_err("editor buffer is unavailable"))
    }

    fn validate_call(&self) -> PyResult<()> {
        self.validate_thread()?;
        if self.is_closed {
            return Err(PyRuntimeError::new_err("editor is closed"));
        }
        if self.is_in_callback {
            return Err(PyRuntimeError::new_err(
                "editor calls are rejected inside its change callback",
            ));
        }
        Ok(())
    }

    fn validate_thread(&self) -> PyResult<()> {
        if std::thread::current().id() != self.owner_thread {
            Err(PyRuntimeError::new_err(
                "editor called from a non-owner thread",
            ))
        } else {
            Ok(())
        }
    }
}

fn engine_error(error: continuity_engine::Error) -> PyErr {
    match error {
        continuity_engine::Error::Text(_) | continuity_engine::Error::InvalidArgument { .. } => {
            PyValueError::new_err(error.to_string())
        }
        _ => PyRuntimeError::new_err(error.to_string()),
    }
}

/// Native module loaded by the pure-Python package.
#[pymodule]
fn _continuity_editor(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Editor>()?;
    module.add_class::<Snapshot>()?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
