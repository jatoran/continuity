//! Errors for app-level wiring outside `main.rs`.

use thiserror::Error;

/// Errors emitted by the multi-window registry.
#[derive(Debug, Error)]
pub enum Error {
    /// Sending a spawn request into the registry failed.
    #[error("registry channel closed")]
    RegistryClosed,
    /// Spawning a window thread failed.
    #[error("spawn continuity-window thread: {0}")]
    SpawnThread(#[from] std::io::Error),
    /// COM initialization failed on a window thread.
    #[error(transparent)]
    Win(#[from] continuity_win::Error),
    /// Keymap load failed.
    #[error(transparent)]
    Keymap(#[from] continuity_keymap::Error),
    /// UI construction or run failed.
    #[error(transparent)]
    Ui(#[from] continuity_ui::Error),
    /// Reading a user keymap failed.
    #[error("reading {path}: {source}")]
    ReadKeymap {
        /// Keymap path.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A persistence-thread call failed during initial-request building
    /// or related app-level wiring. The `context` string names the
    /// operation that was in flight so the bubbled message stays
    /// debuggable without dragging `anyhow` outside `main.rs`.
    #[error("{context}: {source}")]
    Persist {
        /// Short description of the in-flight persistence operation.
        context: String,
        /// Underlying persistence error.
        #[source]
        source: continuity_persist::Error,
    },
    /// Resolving the current executable path failed.
    #[error("resolving current executable path: {0}")]
    CurrentExecutable(#[source] std::io::Error),
    /// The current executable path had no parent directory.
    #[error("current executable has no parent directory")]
    CurrentExecutableMissingParent,
}
