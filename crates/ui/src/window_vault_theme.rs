//! Vault-local base theme and allowlisted color overlays.

use crate::window::Window;
use crate::window_helpers::invalidate_hwnd_with_reason;

impl Window {
    pub(crate) fn apply_vault_appearance(
        &mut self,
        config: Option<&continuity_config::VaultConfig>,
    ) {
        let Some(config) = config else {
            if let Some(theme) = self.vault.take_theme_base() {
                self.active_theme.current = theme;
                self.sync_titlebar_theme();
                invalidate_hwnd_with_reason(self.hwnd, "vault_theme_clear");
            }
            return;
        };
        if self.vault.theme_base().is_none() {
            self.vault.set_theme_base(self.active_theme.current.clone());
        }
        let inherited = self
            .vault
            .theme_base()
            .cloned()
            .unwrap_or_else(|| self.active_theme.current.clone());
        let themes_dir = self
            .live_reload
            .as_ref()
            .map(|reload| reload.themes_dir.as_path());
        let mut theme = if config.appearance.theme.trim().is_empty() {
            inherited
        } else {
            crate::window_theme_apply::resolve_theme_by_name(
                themes_dir,
                config.appearance.theme.trim(),
            )
            .unwrap_or(inherited)
        };
        for (key, value) in &config.appearance.colors {
            if theme.colors.contains_key(key) {
                if let Ok(color) = value.parse::<continuity_theme::Color>() {
                    theme.colors.insert(key.clone(), color);
                }
            }
        }
        self.active_theme.current = theme;
        self.sync_titlebar_theme();
        invalidate_hwnd_with_reason(self.hwnd, "vault_theme_apply");
    }
}
