//! Keymap reload + conflict-log helpers split out of
//! [`crate::window_commanding`] to keep that file under the 600-line
//! cap (Phase H6 split, 2026-05-13).
//!
//! `Window::reload_keymap_from_sources` is the runtime hot-reload entry
//! point; `Window::log_keymap_conflicts` writes the conflict list to
//! stderr after a keymap reload.
//!
//! Thread ownership: UI thread of the owning [`crate::Window`].

use continuity_keymap::Keymap;

use crate::Window;

impl Window {
    pub(crate) fn log_keymap_conflicts(&self) {
        if self.keymap_conflicts.is_empty() {
            eprintln!("continuity-keymap: no conflicts");
            return;
        }
        for conflict in &self.keymap_conflicts {
            match &conflict.when {
                Some(when) => eprintln!(
                    "continuity-keymap: conflict `{}` maps both `{}` and `{}` when `{}`",
                    conflict.chord, conflict.a, conflict.b, when
                ),
                None => eprintln!(
                    "continuity-keymap: conflict `{}` maps both `{}` and `{}`",
                    conflict.chord, conflict.a, conflict.b
                ),
            }
        }
    }

    pub(crate) fn reload_keymap_from_sources(&mut self) -> Result<(), continuity_command::Error> {
        let base = Keymap::from_toml(self.default_keymap_toml).map_err(|e| {
            continuity_command::Error::InvalidArgs {
                name: "keymap.reload",
                reason: format!("default keymap failed to parse: {e}"),
            }
        })?;
        let keymap = match &self.user_keymap_path {
            Some(path) if path.exists() => {
                let toml = std::fs::read_to_string(path).map_err(|e| {
                    continuity_command::Error::InvalidArgs {
                        name: "keymap.reload",
                        reason: format!("failed to read {}: {e}", path.display()),
                    }
                })?;
                let user = Keymap::from_toml(&toml).map_err(|e| {
                    continuity_command::Error::InvalidArgs {
                        name: "keymap.reload",
                        reason: format!("failed to parse {}: {e}", path.display()),
                    }
                })?;
                Keymap::layered(base, user)
            }
            _ => base,
        };
        self.keymap = keymap;
        self.refresh_keymap_conflicts();
        self.log_keymap_conflicts();
        Ok(())
    }
}
