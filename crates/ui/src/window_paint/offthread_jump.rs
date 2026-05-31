//! Off-thread realization arm for large viewport jumps.
//!
//! Thread ownership: mutates `Window::jump_offthread_polls` on the UI
//! thread and submits a best-effort projection-worker request.

use crate::window::Window;

/// Off-thread big-jump realization poll budget. A Ctrl+End / Ctrl+Home
/// or far reveal jump arms [`Window::jump_offthread_polls`] to this many
/// cheap placeholder polls while the projection worker builds the
/// destination off the UI thread.
pub(crate) const JUMP_OFFTHREAD_MAX_POLLS: u8 = 6;

impl Window {
    /// Arm off-thread realization for a viewport jump.
    ///
    /// Submits a projection-worker request for the just-jumped-to viewport
    /// and sets the placeholder-poll budget so subsequent paints reuse the
    /// prior frame plus a placeholder strip while the worker builds. Call
    /// after `self.view` has moved to the target so prewarm targets the
    /// destination viewport.
    pub(crate) fn arm_offthread_jump(&mut self, reason: &'static str) {
        let _ = self.try_dispatch_projection_worker_early(reason, "focus_change");
        self.jump_offthread_polls = JUMP_OFFTHREAD_MAX_POLLS;
    }
}
