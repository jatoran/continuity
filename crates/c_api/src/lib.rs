#![warn(missing_docs)]
//! Versioned C ABI for the synchronous Continuity editor engine.

mod api;
mod error;
mod handle;
mod types;

pub use api::{
    continuity_engine_capabilities, continuity_engine_carets, continuity_engine_carets_free,
    continuity_engine_create_utf16, continuity_engine_create_utf8,
    continuity_engine_delete_backward, continuity_engine_deltas_free,
    continuity_engine_deltas_since, continuity_engine_destroy, continuity_engine_insert_utf16,
    continuity_engine_insert_utf8, continuity_engine_last_error_utf8, continuity_engine_redo,
    continuity_engine_redo_alternate, continuity_engine_revision, continuity_engine_set_carets,
    continuity_engine_set_change_callback, continuity_engine_snapshot_utf16,
    continuity_engine_snapshot_utf8, continuity_engine_string_free, continuity_engine_undo,
    continuity_engine_utf16_string_free,
};
pub use types::{
    ContinuityEngineCapabilities, ContinuityEngineChangeCallback, ContinuityEngineDelta,
    ContinuityEngineHandle, ContinuityEnginePosition, ContinuityEngineStatus,
    ContinuityEngineString, ContinuityEngineUtf16String, CONTINUITY_ENGINE_ABI_MAJOR,
    CONTINUITY_ENGINE_ABI_MINOR, CONTINUITY_ENGINE_CAP_BRANCHING_UNDO,
    CONTINUITY_ENGINE_CAP_CALLBACK, CONTINUITY_ENGINE_CAP_MULTI_CURSOR,
    CONTINUITY_ENGINE_CAP_UTF16, CONTINUITY_ENGINE_SDK_MAJOR, CONTINUITY_ENGINE_SDK_MINOR,
    CONTINUITY_ENGINE_SDK_PATCH,
};
