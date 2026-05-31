//! Errors for the `continuity-command` crate.

use thiserror::Error;

/// Errors that can arise during command dispatch.
#[derive(Debug, Error)]
pub enum Error {
    /// The named command is not registered.
    #[error("unknown command `{0}`")]
    UnknownCommand(String),

    /// A command's context predicate evaluated false.
    #[error("command `{0}` not applicable in current context")]
    NotApplicable(String),

    /// Command arguments failed to parse.
    #[error("invalid args for `{name}`: {reason}")]
    InvalidArgs {
        /// The command id.
        name: &'static str,
        /// Human-readable reason.
        reason: String,
    },

    /// The active context does not support the operation required by a
    /// command.
    #[error("context does not support `{0}`")]
    UnsupportedContext(&'static str),

    /// The editor core rejected a command operation.
    #[error(transparent)]
    Core(#[from] continuity_core::Error),

    /// A command-handler-specific failure that doesn't fit any other
    /// variant (e.g. `ShellExecuteW` returned an error code, or a
    /// configured external editor isn't installed).
    #[error("command failed: {0}")]
    Other(String),
}
