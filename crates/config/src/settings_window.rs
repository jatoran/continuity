//! `[window]` section of `settings.toml`, extracted from
//! [`crate::settings`] so that file stays under the 600-line cap.

use serde::Deserialize;

/// `[window]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Restore each window to its remembered virtual desktop on launch.
    pub restore_to_virtual_desktops: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            restore_to_virtual_desktops: true,
        }
    }
}
