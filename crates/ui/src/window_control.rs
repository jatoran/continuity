//! Phase-16.5 cross-thread control channel from `app::registry` to a
//! [`crate::Window`].
//!
//! Thread ownership: the registry main loop is the sole sender; each
//! window's UI thread is the sole receiver. The window drains its
//! receiver from a `WM_TIMER` tick (the same tick that used to drain the
//! [`continuity_config::SettingsWatcher`] directly).
//!
//! Variants are intentionally narrow — generic event buses encourage
//! cross-layer coupling. Add a typed variant per concrete control flow.

use continuity_buffer::{BufferId, FileAssociation};
use continuity_config::ConfigEvent;
use continuity_persist::PersistEvent;
use crossbeam_channel::{Receiver, Sender};

/// One typed control message routed from the registry to a window.
#[derive(Clone, Debug)]
pub enum WindowControl {
    /// Live-reloaded settings / keymap / theme. Same payload the
    /// [`continuity_config::SettingsWatcher`] emits, fanned out to every
    /// live window by the registry.
    ConfigChanged(ConfigEvent),
    /// δ.3 — a persistence-thread event the registry observed (write
    /// failure or clean shutdown). Each live window banners these so
    /// the "saving = export" durability promise stays visible when the
    /// underlying writer is unhealthy. The registry also synthesizes
    /// [`PersistEvent::ThreadStopped`] when its receiver disconnects
    /// (the persist thread panicked rather than exited cleanly).
    PersistEvent(PersistEvent),
    /// A newer release is available; show the offer banner.
    UpdateAvailable(UpdateOffer),
    /// Progress or outcome text from the update host ("Downloading…",
    /// "Continuity is up to date", a failure). `sticky` keeps it until
    /// the user dismisses it or the app exits to install.
    UpdateStatus {
        /// Banner text.
        text: String,
        /// Whether the banner stays until dismissed.
        sticky: bool,
    },
    /// Reveal (and focus) an already-open file buffer in this window, in
    /// response to a reopen of a path the registry routed here. The window
    /// activates an existing tab for the buffer (or adopts a fresh tab if
    /// the tab was closed but the buffer is still alive), brings itself to
    /// the foreground, and reconciles the buffer against the freshly-read
    /// disk bytes. This is how reopening a file focuses the existing tab
    /// instead of spawning a duplicate window.
    RevealBufferTab {
        /// The buffer to surface.
        buffer_id: BufferId,
        /// Current decoded disk content (for reconciliation).
        content: String,
        /// Current filesystem association (mtime + raw/content hashes).
        file: FileAssociation,
        /// Launch-time banners to surface after revealing (e.g. an
        /// encoding notice from a forwarded open). Usually empty.
        notices: Vec<String>,
    },
    /// Open a freshly resolved buffer in the requesting window.
    OpenBufferTab {
        /// Canonical shared buffer.
        buffer_id: BufferId,
        /// Current disk content for reconciliation.
        content: String,
        /// Current filesystem association.
        file: FileAssociation,
        /// Preview or permanent-tab semantics.
        disposition: crate::window_config::FileOpenDisposition,
        /// Stable destination pane captured when the open was requested.
        target_pane: Option<crate::pane_tree::PaneId>,
        /// Notices raised by the file decoder.
        notices: Vec<String>,
    },
}

/// Sender end of a registry → window control channel. Owned by the
/// registry main loop.
pub type WindowControlTx = Sender<WindowControl>;

/// A newer release the host found on GitHub Releases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateOffer {
    /// Version string without the `v` prefix, e.g. `0.4.12`.
    pub version: String,
    /// Release page (`html_url`) for the *Release notes* button.
    pub notes_url: String,
}

/// What the user asked the host to do about updates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateAction {
    /// Download, verify, and install the offered release, then exit.
    Install(UpdateOffer),
    /// Never offer this version again.
    Skip(String),
    /// Poll GitHub Releases now (`help.check_for_updates`).
    CheckNow,
}

/// Host callback that carries an [`UpdateAction`] off the UI thread.
#[derive(Clone)]
pub struct UpdateActions(pub std::sync::Arc<dyn Fn(UpdateAction) + Send + Sync>);

impl std::fmt::Debug for UpdateActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UpdateActions(<host callback>)")
    }
}

/// Receiver end of a registry → window control channel. Owned by a
/// single window's UI thread.
pub type WindowControlRx = Receiver<WindowControl>;

/// Ask a window to close gracefully (`WM_CLOSE`) from another thread —
/// the update host uses it to shut every window down before the
/// installer replaces the executable. Every save/autosave path runs on
/// the window's own close sequence exactly as for a user-initiated close.
pub fn request_window_close(raw_window: usize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    if raw_window == 0 {
        return;
    }
    let hwnd = HWND(raw_window as *mut core::ffi::c_void);
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Post an immediate control-channel drain tick to a live window.
///
/// The ordinary 250 ms timer remains as a lost-wake fallback; registry
/// routing calls this after enqueueing latency-sensitive file opens.
pub fn wake_window_control(raw_window: usize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_TIMER};

    if raw_window == 0 {
        return;
    }
    let hwnd = HWND(raw_window as *mut core::ffi::c_void);
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            WM_TIMER,
            WPARAM(crate::window_timers::CONFIG_POLL_TIMER_ID),
            LPARAM(0),
        );
    }
}
