//! Normalized editor-surface input.

use continuity_buffer::BufferId;
use continuity_text::{Position, Selection};

use crate::OperationRequest;

/// Which subsystem owns a named command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandTarget {
    /// Portable editor state or projection behavior.
    Editor,
    /// Desktop files, panes, tabs, windows, settings, or application state.
    DesktopHost,
    /// An embedding host registered the command.
    EmbeddingHost,
}

/// Logical navigation unit independent of physical keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationUnit {
    /// One grapheme cluster.
    Grapheme,
    /// One word boundary.
    Word,
    /// One visual display row.
    DisplayRow,
    /// One source line.
    SourceLine,
    /// Start or end of a source line.
    LineBoundary,
    /// Start or end of the document.
    DocumentBoundary,
    /// One viewport page.
    Page,
}

/// Logical caret movement after key or accessibility translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationIntent {
    /// Target buffer.
    pub buffer_id: BufferId,
    /// Unit of movement.
    pub unit: NavigationUnit,
    /// Negative moves backward/up; positive moves forward/down.
    pub direction: i8,
    /// Extend selections instead of collapsing/moving them.
    pub extend: bool,
}

/// Selection replacement independent of pointer or keyboard source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionIntent {
    /// Target buffer.
    pub buffer_id: BufferId,
    /// Canonical source selections.
    pub selections: Vec<Selection>,
}

/// Logical viewport geometry in device-independent pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    /// Width in DIPs.
    pub width_dip: f32,
    /// Height in DIPs.
    pub height_dip: f32,
    /// Device pixels per DIP.
    pub scale: f32,
}

/// Scroll update in device-independent pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollIntent {
    /// Horizontal delta.
    pub delta_x_dip: f32,
    /// Vertical delta.
    pub delta_y_dip: f32,
    /// `true` when motion is inertial rather than direct.
    pub is_inertial: bool,
}

/// Focus lifecycle input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusIntent {
    /// Editor surface gained focus.
    Gained,
    /// Editor surface lost focus.
    Lost,
}

/// Platform-neutral pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    /// No button, used for hover/move.
    None,
    /// Primary action button.
    Primary,
    /// Secondary action button.
    Secondary,
    /// Middle button.
    Middle,
}

/// Lifecycle phase of normalized pointer input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    /// A pointer button was pressed.
    Down,
    /// A pointer button was released.
    Up,
    /// Pointer coordinates or held-button state changed.
    Move,
    /// Pointer left the surface bounds.
    Leave,
    /// Platform capture was lost and any surface drag must end.
    Cancel,
}

/// Pointer input after host coordinate translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerIntent {
    /// X position in surface DIPs.
    pub x_dip: f32,
    /// Y position in surface DIPs.
    pub y_dip: f32,
    /// Active or changed button.
    pub button: PointerButton,
    /// Down, up, motion, leave, or capture-cancellation phase.
    pub phase: PointerPhase,
    /// Click count reported by the host.
    pub click_count: u8,
    /// Whether the primary button is held after this event.
    pub is_primary_down: bool,
    /// Whether the secondary button is held after this event.
    pub is_secondary_down: bool,
    /// Whether the middle button is held after this event.
    pub is_middle_down: bool,
    /// Whether Shift is held.
    pub is_shift_down: bool,
    /// Whether Control is held.
    pub is_control_down: bool,
    /// Whether Alt/Option is held.
    pub is_alt_down: bool,
}

/// Text composition lifecycle after native IME translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositionIntent {
    /// Start a composition at the canonical source position.
    Start {
        /// Canonical source insertion position.
        position: Position,
    },
    /// Replace current preedit text. Selection offsets are UTF-16 code units.
    Update {
        /// Current preedit string.
        text: String,
        /// Selected UTF-16 range inside `text`.
        selection_utf16: std::ops::Range<u32>,
    },
    /// Commit composition text as ordinary editor input.
    Commit(String),
    /// Cancel without committing.
    Cancel,
}

/// Requests an editor surface can direct to its embedding host.
#[derive(Clone, Debug, PartialEq)]
pub enum HostRequest {
    /// Read text from the host clipboard.
    ReadClipboard,
    /// Write text to the host clipboard.
    WriteClipboard(String),
    /// Open a context menu at surface coordinates.
    ContextMenu {
        /// Horizontal surface coordinate.
        x_dip: f32,
        /// Vertical surface coordinate.
        y_dip: f32,
    },
    /// Activate a URL or document anchor.
    ActivateLink(String),
    /// Files dropped on the surface, expressed as host-platform paths.
    DroppedFiles(Vec<String>),
}

/// Complete platform-neutral input vocabulary.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorIntent {
    /// Typed engine mutation or selection update.
    Operation(OperationRequest),
    /// Logical navigation.
    Navigate(NavigationIntent),
    /// Canonical selection replacement.
    Select(SelectionIntent),
    /// Named command after keymap resolution.
    DispatchCommand {
        /// Stable command name.
        name: String,
        /// Owning subsystem.
        target: CommandTarget,
    },
    /// Surface viewport changed.
    ViewportChanged(Viewport),
    /// Surface scrolled.
    Scroll(ScrollIntent),
    /// Surface focus changed.
    Focus(FocusIntent),
    /// Pointer input.
    Pointer(PointerIntent),
    /// Composition lifecycle input.
    Composition(CompositionIntent),
    /// Explicit mediation request to the embedding host.
    Request(HostRequest),
}
