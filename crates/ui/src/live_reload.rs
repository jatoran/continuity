//! Per-window settings live-reload bundle.
//!
//! `LiveReload` is the data type each `Window` receives at construction:
//! paths it needs for `settings.open`, the persist-mode applier closure,
//! and a shared cell carrying the **latest committed** [`Settings`] as
//! observed by the registry. New-window construction reads the cell on
//! `maybe_apply_initial_settings`, which is what lets a theme (or any
//! other settings) commit propagate to windows opened *after* the
//! commit landed.
//!
//! The `Window`-impl apply path that consumes `LiveReload` lives in
//! `window_settings_reload.rs`. This module deliberately carries only
//! the data + access helpers so the apply path stays under the 600-line
//! conventions cap and the cross-thread access rules are stated once,
//! next to the cell.
//!
//! Thread ownership: the cell is written by the registry's main thread
//! (one writer per settings-watcher event), and read by every UI thread
//! at window construction (one reader per spawn). Both operations
//! complete in microseconds, so a `std::sync::Mutex` is sufficient even
//! though contention is in principle multi-thread — there is no
//! sustained lock-held path.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use continuity_config::Settings;

/// Callback invoked when `[persistence].mode` changes. The argument is
/// the SQLite `PRAGMA synchronous` value derived from the new mode
/// (one of `"NORMAL" | "FULL" | "OFF"`).
///
/// The `app` crate wires this to `PersistClient::set_synchronous` —
/// using a closure here keeps `ui` independent of the persist crate.
pub(crate) type SyncModeApplier = Arc<dyn Fn(&str) + Send + Sync>;

/// Bundle handed to every [`crate::Window`] so it can apply the user's
/// settings on launch and surface `settings.open` in the palette.
///
/// `initial` is **shared** across all windows: it carries the latest
/// `Settings` the registry has observed (either the bootstrap snapshot
/// from disk at process start, or the most recent `ConfigEvent::Settings`
/// payload from the file-watcher). A commit-from-window-A → watcher →
/// registry → cell-replace sequence makes the new state visible to any
/// new window that subsequently spawns, fixing the regression where
/// windows opened *after* a runtime theme commit replayed the
/// process-start snapshot and ignored the commit.
///
/// `Mutex` justification (per the project's "Mutex only with a
/// justifying comment" rule): the cell is replaced wholesale on each
/// settings event, so finer-grained locks would buy nothing; readers
/// only lock long enough to clone a `Settings` value out (microseconds
/// per spawn); the writer locks once per file-watcher event. No
/// sustained critical section exists.
#[derive(Clone)]
pub struct LiveReload {
    /// Closure applied whenever the persistence-mode profile changes.
    /// See [`SyncModeApplier`]. The registry calls this directly when
    /// a new settings event arrives; the window applies it once at
    /// launch from the initial settings.
    pub apply_sync_mode: SyncModeApplier,
    /// Path to the user's `settings.toml`. Used by `settings.open`.
    pub settings_path: PathBuf,
    /// Path to the user's `themes/` directory. Theme TOMLs land here
    /// when `cycle_theme`/`theme.reload` resolves a non-bundled name.
    pub themes_dir: PathBuf,
    /// Shared cell carrying the latest committed `Settings` as observed
    /// by the registry. Read by every new-window construction through
    /// [`LiveReload::current_settings`]; replaced by the registry on
    /// each `ConfigEvent::Settings` through
    /// [`LiveReload::replace_settings`]. The field is named `initial`
    /// for historical reasons — its semantic role is "the most recent
    /// commit we've seen", which is also the snapshot a new window
    /// should bootstrap from.
    pub initial: Arc<Mutex<Settings>>,
}

impl LiveReload {
    /// Snapshot the latest committed settings. The lock is held only
    /// long enough to clone; the returned `Settings` is owned by the
    /// caller so subsequent reads can never observe a torn update.
    /// Recovers from a poisoned lock by extracting the inner value —
    /// the cell is wholesale-replaced on each write, so even a
    /// poisoned snapshot is internally consistent.
    #[must_use]
    pub fn current_settings(&self) -> Settings {
        match self.initial.lock() {
            Ok(guard) => guard.clone(),
            Err(poison) => poison.into_inner().clone(),
        }
    }

    /// Replace the cell with `next`. Called by the registry on each
    /// settings-watcher event so subsequent window construction sees
    /// the latest committed value.
    pub fn replace_settings(&self, next: Settings) {
        match self.initial.lock() {
            Ok(mut guard) => *guard = next,
            Err(poison) => *poison.into_inner() = next,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cell(s: Settings) -> LiveReload {
        LiveReload {
            apply_sync_mode: Arc::new(|_| {}),
            settings_path: PathBuf::from("settings.toml"),
            themes_dir: PathBuf::from("themes"),
            initial: Arc::new(Mutex::new(s)),
        }
    }

    #[test]
    fn current_settings_returns_owned_clone() {
        let initial = Settings::default();
        let lr = make_cell(initial.clone());
        let snap = lr.current_settings();
        // Cloned, not the same reference.
        assert_eq!(snap.ui.theme_dark, initial.ui.theme_dark);
    }

    #[test]
    fn replace_settings_updates_subsequent_reads() {
        let lr = make_cell(Settings::default());
        let mut next = Settings::default();
        next.ui.theme_dark = "my_custom".to_string();
        lr.replace_settings(next);
        let snap = lr.current_settings();
        assert_eq!(snap.ui.theme_dark, "my_custom");
    }

    #[test]
    fn replace_visible_through_cloned_handle() {
        // The fix relies on `LiveReload::clone()` sharing the cell:
        // every window thread clones the bundle from the registry
        // ctx, and updates the registry makes must be observable
        // through those clones. Verify the Arc semantics hold.
        let lr_a = make_cell(Settings::default());
        let lr_b = lr_a.clone();
        let mut next = Settings::default();
        next.ui.theme_light = "my_paper".to_string();
        lr_a.replace_settings(next);
        let snap = lr_b.current_settings();
        assert_eq!(snap.ui.theme_light, "my_paper");
    }

    #[test]
    fn replace_then_replace_carries_latest() {
        let lr = make_cell(Settings::default());
        let mut first = Settings::default();
        first.ui.theme_dark = "first".to_string();
        lr.replace_settings(first);
        let mut second = Settings::default();
        second.ui.theme_dark = "second".to_string();
        lr.replace_settings(second);
        assert_eq!(lr.current_settings().ui.theme_dark, "second");
    }
}
