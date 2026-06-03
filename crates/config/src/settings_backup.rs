//! `[backup]` settings section.
//!
//! Split out of `settings.rs` to keep that file under the 600-line cap.
//! Hot-backup cadence and retention live here; the backup task on the
//! persist thread consumes these values at startup.

use serde::Deserialize;

/// `[backup]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackupConfig {
    /// Backup interval in minutes.
    pub interval_minutes: u32,
    /// Hourly retention count.
    pub hourly_retention: u32,
    /// Daily retention count.
    pub daily_retention: u32,
    /// Backup directory (path string; %ENV% is expanded by the consumer).
    pub location: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            interval_minutes: 15,
            hourly_retention: 24,
            daily_retention: 30,
            location: "%LOCALAPPDATA%\\continuity\\backups".into(),
        }
    }
}
