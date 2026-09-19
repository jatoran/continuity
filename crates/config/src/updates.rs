//! `[updates]` settings section — in-app update checking.
//!
//! Pulled out of [`crate::settings`] so that file stays under the
//! 600-line cap.

use serde::Deserialize;

/// `[updates]` section — in-app update checking against GitHub Releases.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig {
    /// Poll GitHub Releases for a newer Continuity and offer it in a
    /// banner. One request shortly after launch and then once a day;
    /// nothing is ever downloaded until the user clicks *Update now*.
    /// Default `true`. Set `false` to never touch the network for updates
    /// (`help.check_for_updates` still works on demand). Read at launch;
    /// a change takes effect on the next start.
    pub check: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self { check: true }
    }
}
