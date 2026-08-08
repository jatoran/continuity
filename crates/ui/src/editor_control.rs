//! Embeddable native Windows editor control.
//!
//! [`EditorControl`] is a real `WS_CHILD` surface. The embedding host owns
//! the parent HWND, message pump, persistence, and application lifetime; the
//! control owns only its synchronous storage-neutral runtime and editor-local
//! rendering/input state.

mod accessibility;
mod input;
mod paint;
mod state;
mod wndproc;

use std::marker::PhantomData;
use std::rc::Rc;

use continuity_buffer::{BufferId, Revision};
use continuity_host::{EditorIntent, HostEventBatch, HostRuntime};
use crossbeam_channel::{Receiver, Sender};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, IsWindow, MoveWindow, ShowWindow, SW_HIDE, SW_SHOW,
};

use crate::editor_control::state::EditorControlState;
use crate::Error;

/// Child-control rectangle in parent-client physical pixels.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ControlBounds {
    /// Left edge relative to the parent client area.
    pub x: i32,
    /// Top edge relative to the parent client area.
    pub y: i32,
    /// Width in physical pixels.
    pub width: i32,
    /// Height in physical pixels.
    pub height: i32,
}

/// How Tab is handled while the child has keyboard focus.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TabBehavior {
    /// Insert an indentation tab through the shared engine.
    #[default]
    InsertIndent,
    /// Move focus to the next or previous sibling dialog tab stop.
    TraverseHost,
}

/// Clipboard ownership for the child control.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlClipboardMode {
    /// Use the native Win32 Unicode clipboard.
    #[default]
    Native,
    /// Emit [`continuity_host::HostRequest`] events for host mediation.
    HostMediated,
}

/// Construction and native-behavior options for one child control.
#[derive(Clone, Debug)]
pub struct ControlOptions {
    /// Whether the child is initially visible.
    pub visible: bool,
    /// Whether soft wrapping follows the child width.
    pub soft_wrap: bool,
    /// How Tab interacts with the parent focus chain.
    pub tab_behavior: TabBehavior,
    /// Native or host-mediated clipboard behavior.
    pub clipboard: ControlClipboardMode,
    /// Whether `WM_DROPFILES` is accepted and forwarded to the host.
    pub accept_file_drop: bool,
    /// DirectWrite family for the editor body.
    pub font_family: String,
    /// Base font size in device-independent pixels.
    pub font_size_dip: f32,
    /// DirectWrite locale.
    pub font_locale: String,
}

impl Default for ControlOptions {
    fn default() -> Self {
        Self {
            visible: true,
            soft_wrap: true,
            tab_behavior: TabBehavior::InsertIndent,
            clipboard: ControlClipboardMode::Native,
            accept_file_drop: false,
            font_family: "Cascadia Mono".to_owned(),
            font_size_dip: 14.0,
            font_locale: "en-us".to_owned(),
        }
    }
}

/// Runtime supplied to a new child control.
pub enum ControlRuntime {
    /// Create a fresh in-memory engine and open the supplied text.
    Ephemeral {
        /// Initial canonical source text.
        initial_text: String,
    },
    /// Adopt a prepared host runtime and one of its open buffers.
    HostRuntime {
        /// Runtime whose owner thread constructs the control.
        runtime: HostRuntime,
        /// Open buffer displayed by the control.
        buffer_id: BufferId,
    },
    /// Adopt a prepared storage-neutral engine and one of its open buffers.
    Engine {
        /// Engine moved into a new [`HostRuntime`] on the current thread.
        engine: continuity_engine::Engine,
        /// Open buffer displayed by the control.
        buffer_id: BufferId,
    },
}

/// Lossless bounded-channel sink for post-dispatch host events.
///
/// Delivery uses backpressure rather than dropping change batches. The host
/// should drain the receiver on another thread or provision enough capacity
/// for its pump cadence.
#[derive(Clone)]
pub struct ControlEventSink {
    sender: Sender<HostEventBatch>,
}

impl ControlEventSink {
    /// Create a bounded lossless sink and its receiving endpoint.
    ///
    /// A zero capacity channel is a rendezvous channel and therefore requires
    /// a concurrently draining receiver.
    #[must_use]
    pub fn bounded(capacity: usize) -> (Self, Receiver<HostEventBatch>) {
        let (sender, receiver) = crossbeam_channel::bounded(capacity);
        (Self { sender }, receiver)
    }

    fn deliver(&self, batch: HostEventBatch) -> Result<(), Error> {
        self.sender
            .send(batch)
            .map_err(|_| Error::HostEventSinkDisconnected)
    }
}

/// Owning Rust handle for one embeddable child editor.
///
/// **Thread ownership:** this handle and all mutable control state belong to
/// the thread that constructs it. The `Rc` marker prevents `Send`/`Sync`.
/// Dropping it destroys only the child HWND; it never posts quit, saves window
/// placement, or touches desktop registries/persistence.
pub struct EditorControl {
    state: Box<EditorControlState>,
    _thread_affinity: PhantomData<Rc<()>>,
}

impl EditorControl {
    /// Create a `WS_CHILD` editor inside `parent`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid parent, runtime/buffer mismatch,
    /// DirectWrite/COM initialization failure, or Win32 creation failure.
    pub fn new(
        parent: HWND,
        bounds: ControlBounds,
        runtime: ControlRuntime,
        options: ControlOptions,
        event_sink: ControlEventSink,
    ) -> Result<Self, Error> {
        if parent.is_invalid() || !unsafe { IsWindow(Some(parent)).as_bool() } {
            return Err(Error::InvalidControlParent);
        }
        let mut state = EditorControlState::create(parent, bounds, runtime, options, event_sink)?;
        state.create_hwnd(bounds)?;
        Ok(Self {
            state,
            _thread_affinity: PhantomData,
        })
    }

    /// Child HWND for parent layout, focus, and host diagnostics.
    #[must_use]
    pub fn hwnd(&self) -> HWND {
        self.state.hwnd
    }

    /// Buffer displayed by this control.
    #[must_use]
    pub fn buffer_id(&self) -> BufferId {
        self.state.buffer_id
    }

    /// Current source revision.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle or runtime error after destruction.
    pub fn revision(&self) -> Result<Revision, Error> {
        self.state.validate_live()?;
        self.state
            .runtime
            .revision(self.state.buffer_id)
            .map_err(Into::into)
    }

    /// Copy current canonical source text.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle or runtime error after destruction.
    pub fn text(&self) -> Result<String, Error> {
        self.state.validate_live()?;
        self.state
            .runtime
            .text(self.state.buffer_id)
            .map_err(Into::into)
    }

    /// Capture immutable canonical text, selections, revision, and read-only
    /// state for host synchronization.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle or runtime error after destruction.
    pub fn snapshot(&self) -> Result<continuity_engine::EngineSnapshot, Error> {
        self.state.validate_live()?;
        self.state
            .runtime
            .snapshot(self.state.buffer_id)
            .map_err(Into::into)
    }

    /// Dispatch normalized input through the shared host/runtime contract.
    ///
    /// # Errors
    ///
    /// Returns a runtime, lifecycle, or event-delivery error.
    pub fn dispatch(&mut self, intent: EditorIntent) -> Result<(), Error> {
        self.state.dispatch(intent)
    }

    /// Dispatch a stable command name. Context-free editor commands become
    /// typed engine operations; all others are emitted for the embedding host.
    ///
    /// # Errors
    ///
    /// Returns a runtime, lifecycle, or event-delivery error.
    pub fn dispatch_command(&mut self, name: &str) -> Result<(), Error> {
        if let Some(operation) = continuity_command::editor_operation_for_command(name) {
            self.state.dispatch_operation(operation)
        } else {
            self.state.dispatch(EditorIntent::DispatchCommand {
                name: name.to_owned(),
                target: continuity_host::CommandTarget::EmbeddingHost,
            })
        }
    }

    /// Supply text returned by a host-mediated clipboard read.
    ///
    /// # Errors
    ///
    /// Returns a runtime, lifecycle, or event-delivery error.
    pub fn provide_clipboard_text(&mut self, text: impl Into<String>) -> Result<(), Error> {
        self.state.insert_text(text.into())
    }

    /// Resize/reposition the child in parent-client physical pixels.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle or Win32 error.
    pub fn set_bounds(&mut self, bounds: ControlBounds) -> Result<(), Error> {
        self.state.validate_live()?;
        unsafe {
            MoveWindow(
                self.state.hwnd,
                bounds.x,
                bounds.y,
                bounds.width.max(1),
                bounds.height.max(1),
                true,
            )?;
        }
        Ok(())
    }

    /// Show or hide the child without changing host window state.
    pub fn set_visible(&mut self, visible: bool) {
        if self.state.is_live {
            unsafe {
                let _ = ShowWindow(self.state.hwnd, if visible { SW_SHOW } else { SW_HIDE });
            }
        }
    }

    /// Enable or disable keyboard/pointer interaction and accessibility state.
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.state.is_live {
            unsafe {
                let _ = EnableWindow(self.state.hwnd, enabled);
            }
            self.state.publish_accessibility();
        }
    }

    /// Give keyboard focus to the editor child.
    pub fn focus(&self) {
        if self.state.is_live {
            unsafe {
                let _ = SetFocus(Some(self.state.hwnd));
            }
        }
    }

    /// Destroy the child. Repeated calls are harmless.
    ///
    /// # Errors
    ///
    /// Returns a Win32 error if destruction fails.
    pub fn destroy(&mut self) -> Result<(), Error> {
        if self.state.is_live {
            unsafe { DestroyWindow(self.state.hwnd)? };
        }
        Ok(())
    }
}

impl Drop for EditorControl {
    fn drop(&mut self) {
        if self.state.is_live {
            unsafe {
                let _ = DestroyWindow(self.state.hwnd);
            }
        }
        let _ = self.state.runtime.close();
    }
}
