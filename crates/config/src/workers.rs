//! Worker-thread settings.

use serde::Deserialize;

/// `[workers]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkerConfig {
    /// Decoration worker watchdog timeout in milliseconds.
    ///
    /// A worker that does not finish its current decoration request
    /// within this window is abandoned and replaced; the in-flight
    /// request is re-enqueued against the replacement pool.
    pub decoration_watchdog_ms: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            decoration_watchdog_ms: 2_000,
        }
    }
}
