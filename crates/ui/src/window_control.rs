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
}

/// Sender end of a registry → window control channel. Owned by the
/// registry main loop.
pub type WindowControlTx = Sender<WindowControl>;

/// Receiver end of a registry → window control channel. Owned by a
/// single window's UI thread.
pub type WindowControlRx = Receiver<WindowControl>;
