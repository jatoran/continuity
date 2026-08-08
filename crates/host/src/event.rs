//! Revisioned events delivered after a dispatch releases the engine borrow.

use continuity_buffer::{BufferId, Revision};
use continuity_engine::ChangeBatch;
use continuity_text::Selection;

use crate::{
    CompositionIntent, FocusIntent, HostRequest, NavigationIntent, PointerIntent, Viewport,
};

/// Why a surface must redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalidation {
    /// Source text or projection inputs changed.
    Content,
    /// Selection or caret changed.
    Selection,
    /// Viewport geometry or scroll changed.
    Viewport,
    /// Focus or composition visuals changed.
    InputState,
}

/// Severity and presentation class for a recoverable problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    /// Informational status.
    Information,
    /// Recoverable warning.
    Warning,
    /// Operation failed but the editor remains usable.
    Error,
}

/// One post-dispatch notification to an embedding host or surface adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    /// A complete text mutation is available for host persistence.
    Change(Box<ChangeBatch>),
    /// Canonical selection state after an operation.
    SelectionChanged {
        /// Affected buffer.
        buffer_id: BufferId,
        /// Current source revision.
        revision: Revision,
        /// Complete normalized selection set.
        selections: Vec<Selection>,
    },
    /// Focus state changed.
    FocusChanged(FocusIntent),
    /// Viewport geometry changed.
    ViewportChanged(Viewport),
    /// A redraw is required.
    Invalidate(Invalidation),
    /// Clipboard, context-menu, or link mediation request.
    HostRequest(HostRequest),
    /// A logical navigation request for the surface controller.
    NavigationRequested(NavigationIntent),
    /// A named command for its declared owner.
    CommandRequested {
        /// Stable command name.
        name: String,
        /// Owning subsystem.
        target: crate::CommandTarget,
    },
    /// Pointer input for hit testing by the projection controller.
    Pointer(PointerIntent),
    /// Composition state for the text-input adapter.
    Composition(CompositionIntent),
    /// Recoverable error data displayed as a non-modal banner.
    Banner {
        /// Severity/presentation class.
        kind: BannerKind,
        /// Stable machine-readable code.
        code: String,
        /// User-facing message.
        message: String,
    },
}

/// Events caused by exactly one [`crate::HostRuntime::dispatch`] call.
///
/// Batches are strictly ordered by `sequence`. Hosts must not reorder or
/// partially deliver events within a batch.
#[derive(Clone, Debug, PartialEq)]
pub struct HostEventBatch {
    /// Monotonic runtime-local sequence, starting at one.
    pub sequence: u64,
    /// Events in causal order.
    pub events: Vec<HostEvent>,
}
