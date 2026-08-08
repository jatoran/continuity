#![warn(missing_docs)]
//! Platform-neutral contracts between an editor surface and its host.
//!
//! One [`HostRuntime::dispatch`] call completes synchronously and returns one
//! [`HostEventBatch`]. No host callback is invoked while the engine is
//! borrowed. Callers deliver the returned batch only after `dispatch` returns.

mod command;
mod error;
mod event;
mod intent;
mod operation;
mod runtime;
mod utf16;

pub use command::editor_operation_for_command;
pub use error::Error;
pub use event::{BannerKind, HostEvent, HostEventBatch, Invalidation};
pub use intent::{
    CommandTarget, CompositionIntent, EditorIntent, FocusIntent, HostRequest, NavigationIntent,
    NavigationUnit, PointerButton, PointerIntent, PointerPhase, ScrollIntent, SelectionIntent,
    Viewport,
};
pub use operation::{apply_editor_operation, EditorOperation, OperationRequest, OperationResult};
pub use runtime::HostRuntime;
pub use utf16::{byte_to_utf16, utf16_to_byte};
