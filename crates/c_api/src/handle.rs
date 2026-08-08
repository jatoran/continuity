//! One-document engine handle and callback boundary.

use std::ffi::c_void;
use std::thread::ThreadId;

use continuity_buffer::{BufferId, Revision};
use continuity_engine_core::{Engine, SelectionEdit};
use continuity_text::{Position, Selection};

use crate::error::AbiError;
use crate::{
    ContinuityEngineChangeCallback, ContinuityEngineHandle, ContinuityEnginePosition,
    ContinuityEngineStatus,
};

/// Mutable engine state owned by the thread that constructed it.
pub(crate) struct Handle {
    engine: Engine,
    buffer_id: BufferId,
    owner_thread: ThreadId,
    callback: ContinuityEngineChangeCallback,
    callback_user_data: *mut c_void,
    is_in_callback: bool,
}

impl Handle {
    pub(crate) fn new(text: &str, revision: u64) -> Self {
        let mut engine = Engine::new();
        let buffer_id = BufferId::new();
        engine.load_buffer(buffer_id, text, Revision(revision));
        engine.drain_events();
        Self {
            engine,
            buffer_id,
            owner_thread: std::thread::current().id(),
            callback: None,
            callback_user_data: std::ptr::null_mut(),
            is_in_callback: false,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), AbiError> {
        if std::thread::current().id() != self.owner_thread {
            return Err(AbiError::new(
                ContinuityEngineStatus::WrongThread,
                "engine handle called from a non-owner thread",
            ));
        }
        if self.is_in_callback {
            return Err(AbiError::new(
                ContinuityEngineStatus::ReentrantCall,
                "engine handle calls are rejected inside its change callback",
            ));
        }
        Ok(())
    }

    pub(crate) fn set_carets(
        &mut self,
        positions: &[ContinuityEnginePosition],
    ) -> Result<(), AbiError> {
        let selections = positions
            .iter()
            .map(|position| {
                Selection::caret_at(Position::new(position.line, position.byte_in_line))
            })
            .collect();
        self.engine
            .set_selections(self.buffer_id, selections)
            .map_err(engine_error)
    }

    pub(crate) fn apply_selection_edit(
        &mut self,
        edit: SelectionEdit,
        timestamp_ms: i64,
    ) -> Result<Option<u64>, AbiError> {
        let batch = self
            .engine
            .apply_selection_edit(self.buffer_id, &edit, timestamp_ms)
            .map_err(engine_error)?;
        Ok(batch.map(|batch| batch.revision_after.get()))
    }

    pub(crate) fn undo(&mut self, timestamp_ms: i64) -> Result<Option<u64>, AbiError> {
        self.engine
            .undo(self.buffer_id, timestamp_ms)
            .map(|batch| batch.map(|batch| batch.revision_after.get()))
            .map_err(engine_error)
    }

    pub(crate) fn redo(
        &mut self,
        timestamp_ms: i64,
        is_alternate: bool,
    ) -> Result<Option<u64>, AbiError> {
        let result = if is_alternate {
            self.engine.redo_alternate(self.buffer_id, timestamp_ms)
        } else {
            self.engine.redo(self.buffer_id, timestamp_ms)
        };
        result
            .map(|batch| batch.map(|batch| batch.revision_after.get()))
            .map_err(engine_error)
    }

    pub(crate) fn text(&self) -> Result<String, AbiError> {
        self.engine
            .text(self.buffer_id)
            .ok_or_else(|| engine_error(continuity_engine_core::Error::UnknownBuffer))
    }

    pub(crate) fn revision(&self) -> Result<u64, AbiError> {
        self.engine
            .revision(self.buffer_id)
            .map(Revision::get)
            .ok_or_else(|| engine_error(continuity_engine_core::Error::UnknownBuffer))
    }

    pub(crate) fn carets(&self) -> Result<Vec<ContinuityEnginePosition>, AbiError> {
        let selections = self
            .engine
            .selections(self.buffer_id)
            .ok_or_else(|| engine_error(continuity_engine_core::Error::UnknownBuffer))?;
        Ok(selections
            .iter()
            .map(|selection| ContinuityEnginePosition {
                line: selection.head.line,
                byte_in_line: selection.head.byte_in_line,
            })
            .collect())
    }

    pub(crate) fn deltas(&self, since_revision: u64) -> Vec<crate::ContinuityEngineDelta> {
        self.engine
            .deltas_since(self.buffer_id, since_revision)
            .0
            .into_iter()
            .map(|delta| crate::ContinuityEngineDelta {
                at: delta.at,
                removed_bytes: delta.removed_bytes,
                inserted_bytes: delta.inserted_bytes,
            })
            .collect()
    }
}

pub(crate) unsafe fn checked_handle_mut<'a>(
    handle: *mut ContinuityEngineHandle,
) -> Result<&'a mut Handle, AbiError> {
    let wrapper = unsafe { handle.as_mut() }.ok_or_else(null_error)?;
    wrapper.0.validate()?;
    Ok(&mut wrapper.0)
}

pub(crate) unsafe fn checked_handle<'a>(
    handle: *const ContinuityEngineHandle,
) -> Result<&'a Handle, AbiError> {
    let wrapper = unsafe { handle.as_ref() }.ok_or_else(null_error)?;
    wrapper.0.validate()?;
    Ok(&wrapper.0)
}

pub(crate) unsafe fn notify_change(handle: *mut ContinuityEngineHandle, revision: Option<u64>) {
    let Some(revision) = revision else { return };
    let Some(wrapper) = (unsafe { handle.as_mut() }) else {
        return;
    };
    let callback = wrapper.0.callback;
    let user_data = wrapper.0.callback_user_data;
    let Some(callback) = callback else { return };
    wrapper.0.is_in_callback = true;
    unsafe { callback(user_data, revision) };
    if let Some(wrapper) = unsafe { handle.as_mut() } {
        wrapper.0.is_in_callback = false;
    }
}

pub(crate) fn null_error() -> AbiError {
    AbiError::new(
        ContinuityEngineStatus::NullPointer,
        "required pointer was null",
    )
}

pub(crate) fn engine_error(error: continuity_engine_core::Error) -> AbiError {
    let status = if matches!(error, continuity_engine_core::Error::Text(_)) {
        ContinuityEngineStatus::InvalidPosition
    } else {
        ContinuityEngineStatus::EngineError
    };
    AbiError::new(status, error.to_string())
}

pub(crate) fn set_callback(
    handle: &mut Handle,
    callback: ContinuityEngineChangeCallback,
    user_data: *mut c_void,
) {
    handle.callback = callback;
    handle.callback_user_data = user_data;
}
