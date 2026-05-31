//! δ.5 theme-management `_impl` bodies on `Window`.
//!
//! These methods carry the file-system + settings-binding logic for the
//! seven theme commands (clone / edit / duplicate / rename / delete /
//! reveal-folder / create-blank). User-supplied names are validated via
//! [`continuity_theme::check_theme_name`]; failure surfaces as a banner
//! and the command no-ops. Successful name commits write the new theme
//! TOML atomically (temp file + rename) so a crash mid-write never leaves
//! a partially-corrupt file. When the affected theme is currently bound
//! in `settings.toml`, the binding is rewritten in place so comments
//! survive.
//!
//! Thread ownership: UI thread of one window. Every method mutates
//! Window-owned state and writes inside `%APPDATA%\continuity\themes\`
//! through synchronous `std::fs` calls — the volume of work per command
//! is bounded by the size of a single TOML file (~10 KiB) and keeps the
//! UI thread responsive.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use continuity_theme::{
    assets::{bundled_named, neutral_fallback, BUNDLED_NAMES},
    check_theme_name, is_reserved_name, NameCheck, Theme,
};
use windows::core::HSTRING;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_D, VK_E};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, SW_SHOWNORMAL};

use crate::theme_picker::ThemeSource;
use crate::window_file::FileBanner;
use crate::window_helpers::invalidate_hwnd;
use crate::window_theme_atomic_write::atomic_write;
use crate::window_theme_settings_edit::{
    update_settings_theme_binding_if, write_settings_theme_binding, ThemeSlot,
};
use crate::Window;

/// Suffix used by `theme.clone` / `theme.duplicate` when the user lets the
/// editor auto-name.
const COPY_SUFFIX: &str = "-copy";

/// Fallback theme name used when `theme.delete` removes the currently-
/// active custom theme. Always one of the bundled names.
const FALLBACK_THEME: &str = "deep_minimal";

impl Window {
    fn active_slot(&self) -> ThemeSlot {
        let resolves_to_dark = matches!(self.active_theme.mode, continuity_theme::Mode::Dark)
            || (matches!(self.active_theme.mode, continuity_theme::Mode::System)
                && self.active_theme.system_dark);
        if resolves_to_dark {
            ThemeSlot::Dark
        } else {
            ThemeSlot::Light
        }
    }

    fn themes_dir(&self) -> Result<PathBuf, continuity_command::Error> {
        self.live_reload
            .as_ref()
            .map(|r| r.themes_dir.clone())
            .ok_or_else(|| {
                continuity_command::Error::Other("themes directory unavailable".to_string())
            })
    }

    fn settings_path(&self) -> Result<PathBuf, continuity_command::Error> {
        self.live_reload
            .as_ref()
            .map(|r| r.settings_path.clone())
            .ok_or_else(|| {
                continuity_command::Error::Other("settings.toml path unavailable".to_string())
            })
    }

    fn set_banner(&mut self, message: impl Into<String>) {
        self.file_banner = Some(FileBanner::new(message.into()));
        invalidate_hwnd(self.hwnd);
    }

    /// δ.5 — clone the currently active theme into a new custom theme.
    pub(crate) fn theme_clone_active_impl(
        &mut self,
        name: Option<&str>,
    ) -> Result<(), crate::Error> {
        let source = self.active_theme.current.clone();
        let source_name = source.name.clone();
        let candidate = match resolve_target_name(name, &source_name, &self.themes_dir()?) {
            Ok(n) => n,
            Err(banner) => {
                self.set_banner(banner);
                return Ok(());
            }
        };
        self.install_custom_theme(&candidate, &source, /*activate=*/ true)
    }

    /// δ.5 — open a theme's TOML for editing. Bundled themes surface a
    /// clone-first banner; custom themes open as a new tab.
    pub(crate) fn theme_edit_impl(&mut self, name: Option<&str>) -> Result<(), crate::Error> {
        let target_name = name
            .map(str::to_string)
            .unwrap_or_else(|| self.active_theme.current.name.clone());
        if is_reserved_name(&target_name) {
            self.set_banner(format!(
                "`{target_name}` is bundled and read-only — clone first to edit",
            ));
            return Ok(());
        }
        let path = self.themes_dir()?.join(format!("{target_name}.toml"));
        if !path.exists() {
            self.set_banner(format!("theme `{target_name}` is not installed on disk"));
            return Ok(());
        }
        self.file_open_paths_impl(vec![path])
            .map_err(crate::Error::Command)
    }

    /// δ.5 — clone any theme (bundled or custom) by name.
    pub(crate) fn theme_duplicate_impl(
        &mut self,
        source: Option<&str>,
        new_name: Option<&str>,
    ) -> Result<(), crate::Error> {
        let themes_dir = self.themes_dir()?;
        let source_name = source
            .map(str::to_string)
            .unwrap_or_else(|| self.active_theme.current.name.clone());
        let theme = match load_theme_by_name(&themes_dir, &source_name) {
            Some(t) => t,
            None => {
                self.set_banner(format!("could not load theme `{source_name}`"));
                return Ok(());
            }
        };
        let candidate = match resolve_target_name(new_name, &source_name, &themes_dir) {
            Ok(n) => n,
            Err(banner) => {
                self.set_banner(banner);
                return Ok(());
            }
        };
        self.install_custom_theme(&candidate, &theme, /*activate=*/ true)
    }

    /// δ.5 — rename a custom theme on disk; update settings binding when
    /// the renamed theme is currently bound.
    pub(crate) fn theme_rename_impl(
        &mut self,
        old: Option<&str>,
        new_name: Option<&str>,
    ) -> Result<(), crate::Error> {
        let themes_dir = self.themes_dir()?;
        let old_name = old
            .map(str::to_string)
            .unwrap_or_else(|| self.active_theme.current.name.clone());
        if is_reserved_name(&old_name) {
            self.set_banner(format!(
                "`{old_name}` is bundled and cannot be renamed — clone first",
            ));
            return Ok(());
        }
        let new = match new_name {
            Some(raw) => match check_theme_name(raw) {
                NameCheck::Ok(n) => n,
                NameCheck::Rejected(reason) => {
                    self.set_banner(reason);
                    return Ok(());
                }
            },
            None => {
                self.set_banner("theme.rename requires a new name");
                return Ok(());
            }
        };
        let old_path = themes_dir.join(format!("{old_name}.toml"));
        let new_path = themes_dir.join(format!("{new}.toml"));
        if !old_path.exists() {
            self.set_banner(format!("theme `{old_name}` is not installed on disk"));
            return Ok(());
        }
        if new_path.exists() {
            self.set_banner(format!("a theme named `{new}` already exists"));
            return Ok(());
        }
        if let Err(e) = std::fs::rename(&old_path, &new_path) {
            self.set_banner(format!("rename failed: {e}"));
            return Ok(());
        }
        // Update settings.toml bindings if either slot pointed at the
        // old name. Failures are non-fatal — we log to the banner so
        // the user can patch manually.
        let settings_path = self.settings_path()?;
        if let Err(e) =
            update_settings_theme_binding_if(&settings_path, |key| key == old_name, &new)
        {
            self.set_banner(format!(
                "theme renamed to `{new}`, but updating settings.toml failed: {e}",
            ));
        }
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// δ.5 — soft-delete a custom theme by moving the file under
    /// `themes/.trash/`. Falls back to a bundled default if the deleted
    /// theme was active.
    pub(crate) fn theme_delete_impl(&mut self, name: Option<&str>) -> Result<(), crate::Error> {
        let themes_dir = self.themes_dir()?;
        let target = name
            .map(str::to_string)
            .unwrap_or_else(|| self.active_theme.current.name.clone());
        if is_reserved_name(&target) {
            self.set_banner(format!("`{target}` is bundled and cannot be deleted"));
            return Ok(());
        }
        let path = themes_dir.join(format!("{target}.toml"));
        if !path.exists() {
            self.set_banner(format!("theme `{target}` is not installed on disk"));
            return Ok(());
        }
        let trash_dir = themes_dir.join(".trash");
        if let Err(e) = std::fs::create_dir_all(&trash_dir) {
            self.set_banner(format!("could not create trash dir: {e}"));
            return Ok(());
        }
        let stamp = unix_ms_now();
        let trash_path = trash_dir.join(format!("{target}-{stamp}.toml"));
        if let Err(e) = std::fs::rename(&path, &trash_path) {
            self.set_banner(format!("delete failed: {e}"));
            return Ok(());
        }
        // If either slot pointed at the deleted theme, swap the binding
        // to the bundled fallback so the next reload paints something
        // valid.
        let was_active = self.active_theme.current.name == target;
        let settings_path = self.settings_path()?;
        if let Err(e) =
            update_settings_theme_binding_if(&settings_path, |key| key == target, FALLBACK_THEME)
        {
            self.set_banner(format!(
                "theme deleted, but updating settings.toml failed: {e}",
            ));
            return Ok(());
        }
        if was_active {
            // Live swap — don't wait for the watcher round-trip.
            if let Ok(theme) = bundled_named(FALLBACK_THEME) {
                let slot = self.active_slot();
                let mut set = self.active_theme.set.clone();
                match slot {
                    ThemeSlot::Dark => set.dark = theme.clone(),
                    ThemeSlot::Light => set.light = theme.clone(),
                }
                self.active_theme.set_installed(set);
            }
            self.set_banner(format!(
                "deleted `{target}`; restored bundled `{FALLBACK_THEME}` (recover from themes/.trash/)",
            ));
        } else {
            self.set_banner(format!("deleted `{target}` (recover from themes/.trash/)",));
        }
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// δ.5 — open the user's themes directory in Explorer.
    pub(crate) fn theme_reveal_folder_impl(&mut self) -> Result<(), crate::Error> {
        let themes_dir = self.themes_dir()?;
        // Make sure the directory exists so Explorer doesn't surface a
        // "folder not found" prompt on first invocation.
        if let Err(e) = std::fs::create_dir_all(&themes_dir) {
            self.set_banner(format!("could not create themes dir: {e}"));
            return Ok(());
        }
        let path_w = HSTRING::from(themes_dir.as_os_str());
        let verb = HSTRING::from("explore");
        let result = unsafe {
            ShellExecuteW(
                Some(self.hwnd),
                &verb,
                &path_w,
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR::null(),
                SHOW_WINDOW_CMD(SW_SHOWNORMAL.0),
            )
        };
        if result.0 as isize <= 32 {
            self.set_banner(format!("ShellExecuteW failed for {}", themes_dir.display(),));
        }
        Ok(())
    }

    /// δ.5 — write a minimal valid theme (neutral fallback values) and
    /// open it for editing.
    pub(crate) fn theme_create_blank_impl(
        &mut self,
        name: Option<&str>,
    ) -> Result<(), crate::Error> {
        let themes_dir = self.themes_dir()?;
        let candidate = match resolve_target_name(name, "custom", &themes_dir) {
            Ok(n) => n,
            Err(banner) => {
                self.set_banner(banner);
                return Ok(());
            }
        };
        let mut theme = neutral_fallback();
        theme.name = candidate.clone();
        self.install_custom_theme(&candidate, &theme, /*activate=*/ false)?;
        // Open the new theme for editing right away — the "blank" flow's
        // whole purpose is to drop the user into the editor with a valid
        // skeleton to tweak.
        let path = themes_dir.join(format!("{candidate}.toml"));
        if path.exists() {
            self.file_open_paths_impl(vec![path])
                .map_err(crate::Error::Command)?;
        }
        Ok(())
    }

    /// Write a custom theme to disk atomically and (optionally) make it
    /// the active theme for the current mode.
    fn install_custom_theme(
        &mut self,
        name: &str,
        source: &Theme,
        activate: bool,
    ) -> Result<(), crate::Error> {
        let themes_dir = self.themes_dir()?;
        if let Err(e) = std::fs::create_dir_all(&themes_dir) {
            self.set_banner(format!("could not create themes dir: {e}"));
            return Ok(());
        }
        let path = themes_dir.join(format!("{name}.toml"));
        let mut theme_copy = source.clone();
        theme_copy.name = name.to_string();
        let body = theme_copy.to_toml();
        if let Err(e) = atomic_write(&path, body.as_bytes()) {
            self.set_banner(format!("write failed: {e}"));
            return Ok(());
        }
        if activate {
            // Swap the freshly-loaded theme into the active slot up front
            // — the watcher will eventually fire `ConfigEvent::Theme` for
            // the same file, but waiting for it would leave the user
            // staring at the old palette for a frame or two.
            let slot = self.active_slot();
            let mut set = self.active_theme.set.clone();
            match slot {
                ThemeSlot::Dark => set.dark = theme_copy.clone(),
                ThemeSlot::Light => set.light = theme_copy.clone(),
            }
            self.active_theme.set_installed(set);
            // Update settings.toml so the binding survives a restart.
            let settings_path = self.settings_path()?;
            if let Err(e) = write_settings_theme_binding(&settings_path, slot, name) {
                self.set_banner(format!(
                    "theme cloned to `{name}` but settings.toml update failed: {e}",
                ));
                return Ok(());
            }
        }
        invalidate_hwnd(self.hwnd);
        self.set_banner(format!("created custom theme `{name}`"));
        Ok(())
    }
}

/// Footer hint string for a theme-picker row, indicating which row-level
/// chords are available. Bundled rows expose `Enter` + `Ctrl+D`; custom
/// rows additionally expose `Ctrl+E` + `Ctrl+Backspace`. Pure data —
/// called by the overlay renderer.
#[must_use]
pub(crate) fn theme_picker_footer_hint(source: &ThemeSource) -> &'static str {
    match source {
        ThemeSource::Bundled => "Enter activate · Ctrl+D duplicate",
        ThemeSource::UserFile(_) => {
            "Enter activate · Ctrl+E edit · Ctrl+D duplicate · Ctrl+Backspace delete"
        }
    }
}

impl Window {
    /// δ.5 theme-picker inline row-action dispatch. Returns `true` when
    /// the chord was consumed. Bundled rows reject edit/delete with a
    /// banner; custom rows fan the action out to the matching command.
    /// Called from `overlay_on_keydown` BEFORE the standard VK_BACK arm
    /// so Ctrl+Backspace isn't routed to the text-input delete path.
    pub(crate) fn theme_picker_row_action(&mut self, vk: u16, _shift: bool) -> bool {
        let Some((name, is_bundled)) = self.theme_picker_row_target() else {
            return false;
        };
        if vk == VK_E.0 {
            // Edit — bundled themes are read-only.
            if is_bundled {
                self.set_banner(format!(
                    "`{name}` is bundled and read-only — clone first to edit",
                ));
                return true;
            }
            // Close picker first so the new tab takes focus cleanly.
            self.dismiss_overlay_and_blur();
            let _ = self.theme_edit_impl(Some(&name));
            return true;
        }
        if vk == VK_D.0 {
            // Duplicate — bundled or custom both supported.
            self.dismiss_overlay_and_blur();
            let _ = self.theme_duplicate_impl(Some(&name), None);
            return true;
        }
        if vk == VK_BACK.0 {
            // Soft-delete — bundled themes refused with a banner; the
            // picker stays open so the user sees the row didn't move.
            if is_bundled {
                self.set_banner(format!("`{name}` is bundled and cannot be deleted"));
                return true;
            }
            self.dismiss_overlay_and_blur();
            let _ = self.theme_delete_impl(Some(&name));
            return true;
        }
        false
    }

    /// Pull the highlighted picker row's (name, is_bundled) tuple, or
    /// `None` when the overlay isn't the theme picker / has no rows.
    fn theme_picker_row_target(&self) -> Option<(String, bool)> {
        let crate::overlays::Overlays::ThemePicker(picker) = &self.overlays else {
            return None;
        };
        let entry = picker
            .filtered
            .get(picker.selected)
            .and_then(|i| picker.all.get(*i))?;
        let is_bundled = matches!(entry.source, ThemeSource::Bundled);
        Some((entry.name.clone(), is_bundled))
    }
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Resolve the user-supplied (or auto-generated) name for a new custom
/// theme. On reject (bundled-name collision, bad characters, length, etc.)
/// returns the banner reason; on accept returns the canonical name and
/// also guarantees no on-disk collision.
fn resolve_target_name(
    requested: Option<&str>,
    source_name: &str,
    themes_dir: &Path,
) -> Result<String, String> {
    let candidate = match requested {
        Some(raw) => match check_theme_name(raw) {
            NameCheck::Ok(n) => n,
            NameCheck::Rejected(reason) => return Err(reason),
        },
        None => auto_name(source_name, themes_dir),
    };
    let path = themes_dir.join(format!("{candidate}.toml"));
    if path.exists() {
        return Err(format!("a theme named `{candidate}` already exists"));
    }
    Ok(candidate)
}

/// Generate `<source>-copy`, `<source>-copy-2`, `<source>-copy-3`, …
/// stopping at the first name that doesn't collide with an existing file
/// or a bundled name.
fn auto_name(source: &str, themes_dir: &Path) -> String {
    let base = format!("{source}{COPY_SUFFIX}");
    let mut candidate = base.clone();
    let mut counter: u32 = 2;
    while themes_dir.join(format!("{candidate}.toml")).exists()
        || BUNDLED_NAMES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&candidate))
    {
        candidate = format!("{base}-{counter}");
        counter = counter.saturating_add(1);
        if counter > 1024 {
            // pathological case: give up and append the timestamp so we
            // never spin forever.
            return format!("{base}-{}", unix_ms_now());
        }
    }
    candidate
}

/// Load a theme by name from either the bundled set or the user themes
/// directory. Bundled themes win when names collide with disk (matches
/// what the picker enumeration does today).
fn load_theme_by_name(themes_dir: &Path, name: &str) -> Option<Theme> {
    if is_reserved_name(name) {
        return bundled_named(name).ok();
    }
    let path = themes_dir.join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path).ok()?;
    Theme::load(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_name_avoids_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("paper-copy.toml"), "x").unwrap();
        std::fs::write(tmp.path().join("paper-copy-2.toml"), "x").unwrap();
        let name = auto_name("paper", tmp.path());
        assert_eq!(name, "paper-copy-3");
    }

    #[test]
    fn auto_name_avoids_bundled_collision() {
        let tmp = tempfile::tempdir().unwrap();
        // `deep_minimal-copy` is unused, so the function should pick it
        // directly without colliding with the bundled `deep_minimal`.
        let name = auto_name("deep_minimal", tmp.path());
        assert_eq!(name, "deep_minimal-copy");
    }

    #[test]
    fn resolve_target_name_rejects_bundled() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve_target_name(Some("deep_minimal"), "paper", tmp.path()).unwrap_err();
        assert!(err.contains("reserved"));
    }

    #[test]
    fn resolve_target_name_rejects_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("existing.toml"), "x").unwrap();
        let err = resolve_target_name(Some("existing"), "paper", tmp.path()).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn footer_hint_distinguishes_bundled_from_custom() {
        let bundled = theme_picker_footer_hint(&ThemeSource::Bundled);
        let custom =
            theme_picker_footer_hint(&ThemeSource::UserFile(PathBuf::from("themes/mine.toml")));
        // Bundled rows never advertise edit or delete.
        assert!(!bundled.contains("Ctrl+E"));
        assert!(!bundled.contains("Backspace"));
        // Custom rows advertise every row-level chord.
        assert!(custom.contains("Ctrl+E"));
        assert!(custom.contains("Ctrl+D"));
        assert!(custom.contains("Backspace"));
    }

    #[test]
    fn footer_hint_always_includes_enter() {
        // Activation is always present.
        assert!(theme_picker_footer_hint(&ThemeSource::Bundled).contains("Enter"));
        assert!(theme_picker_footer_hint(&ThemeSource::UserFile(PathBuf::new())).contains("Enter"));
    }
}
