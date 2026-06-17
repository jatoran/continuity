//! Theme command implementations on `Window`: cycle / reload / pick /
//! apply-entry (preview) / commit-entry (persist) / settings-driven
//! reload apply. Hangs off [`crate::Window`] via `_impl` methods that
//! the `Context` impl in `window_commanding.rs` delegates to.
//!
//! Thread ownership: UI thread (HWND owner). The theme-picker overlay
//! drives live preview through `apply_theme_entry` while the picker is
//! open; the final commit goes through `commit_theme_entry`, which
//! writes the user's selection to `settings.toml`. The config
//! file-watcher then fans the change out to every window — including
//! this one — through `apply_settings_theme_bindings`, which is the
//! single in-memory mutation site for "the chosen theme just changed".
//! This routing is what makes the selection persist across restart
//! (the toml is the source of truth on next launch) and apply to every
//! open window (the watcher's reload echo hits each window's
//! `apply_settings`).

use continuity_config::Settings;
use continuity_theme::Theme;

use crate::theme_picker::ThemeEntry;
use crate::window_file::FileBanner;
use crate::window_helpers::{invalidate_hwnd, invalidate_hwnd_with_reason};
use crate::window_theme_settings_edit::{write_settings_theme_binding, ThemeSlot};
use crate::Window;

impl Window {
    pub(crate) fn reload_theme_impl(&mut self) -> Result<(), crate::Error> {
        // For Phase 11 the bundled themes are the source of truth. The
        // Phase-12 file watcher will swap installed themes via
        // `ActiveTheme::set_installed`.
        let set = continuity_theme::assets::bundled_set().map_err(crate::Error::Theme)?;
        self.active_theme.set_installed(set);
        invalidate_hwnd_with_reason(self.hwnd, "theme_apply");
        Ok(())
    }

    /// §E4 — open the theme-picker palette overlay. Enumerates bundled
    /// themes plus any TOML files in the user's themes directory; live-
    /// previews on highlight; commits on Enter; reverts on Esc.
    pub(crate) fn pick_theme_impl(&mut self) -> Result<(), crate::Error> {
        let themes_dir = self.live_reload.as_ref().map(|r| r.themes_dir.clone());
        let entries = crate::theme_picker::enumerate_themes(themes_dir.as_deref());
        if entries.is_empty() {
            return Ok(());
        }
        let original_name = self.active_theme.current.name.clone();
        let original_set = self.active_theme.set.clone();
        self.overlays
            .open_theme_picker(entries, original_set, original_name);
        self.focus_overlay_input();
        invalidate_hwnd(self.hwnd);
        Ok(())
    }

    /// §E4 helper — load and apply a single theme entry into the
    /// currently-active mode's slot of the `ActiveTheme.set`. **Preview
    /// only**: mutates per-window in-memory state, never writes
    /// settings.toml, never broadcasts. The commit path lives in
    /// `commit_theme_entry`. Errors are logged but never propagate to
    /// the picker (preview must never wedge the editor; the bundled
    /// neutral fallback handles the case where every load fails).
    pub(crate) fn apply_theme_entry(&mut self, entry: &ThemeEntry) {
        let Some(theme) = load_theme_for_entry(entry) else {
            return;
        };
        let slot = active_theme_slot(&self.active_theme);
        let mut next = self.active_theme.set.clone();
        match slot {
            ThemeSlot::Dark => next.dark = theme,
            ThemeSlot::Light => next.light = theme,
        }
        self.active_theme.set_installed(next);
        invalidate_hwnd_with_reason(self.hwnd, "theme_apply");
    }

    /// Final-selection path. Writes the picked theme name into
    /// `[ui].theme_dark` or `[ui].theme_light` (whichever slot the
    /// current mode resolves to) and lets the config file-watcher fan
    /// the change out to every window. The reload echo arrives on the
    /// next config-poll tick and routes through `apply_settings`, which
    /// calls [`Self::apply_settings_theme_bindings`] — the sole
    /// in-memory mutation site for committed (as opposed to previewed)
    /// theme changes. This is what gives us multi-window apply and
    /// persistence across restart for free: settings.toml is the
    /// source of truth.
    pub(crate) fn commit_theme_entry(&mut self, entry: &ThemeEntry) {
        let slot = active_theme_slot(&self.active_theme);
        let settings_path = match self.live_reload.as_ref() {
            Some(r) => r.settings_path.clone(),
            None => {
                crate::paint_trace::log_event(
                    "theme_commit",
                    &format!(
                        "source=picker_keyboard name={} slot={} persisted=false reason=no_settings_path",
                        entry.name,
                        slot_label(slot),
                    ),
                );
                return;
            }
        };
        if let Err(e) = write_settings_theme_binding(&settings_path, slot, &entry.name) {
            self.file_banner = Some(FileBanner::new(format!(
                "theme `{}`: settings write failed: {e}",
                entry.name,
            )));
            invalidate_hwnd(self.hwnd);
            crate::paint_trace::log_event(
                "theme_commit",
                &format!(
                    "source=picker_keyboard name={} slot={} persisted=false err={e}",
                    entry.name,
                    slot_label(slot),
                ),
            );
            return;
        }
        crate::paint_trace::log_event(
            "theme_commit",
            &format!(
                "source=picker_keyboard name={} slot={} persisted=true",
                entry.name,
                slot_label(slot),
            ),
        );
        // Update the shared `LiveReload.initial` cell synchronously so
        // a new-window spawn triggered between this commit and the
        // file-watcher event arriving at the registry observes the new
        // settings on its `maybe_apply_initial_settings` call. The
        // watcher path will fire shortly after and re-replace the
        // cell with the same value (idempotent). On a re-read parse
        // failure the cell is left alone — the watcher will arrive
        // with the authoritative parse result and overwrite then.
        if let Some(reload) = self.live_reload.as_ref() {
            if let Ok(text) = std::fs::read_to_string(&settings_path) {
                if let Ok(snapshot) = continuity_config::Settings::from_toml_validated(&text) {
                    reload.replace_settings(snapshot);
                }
            }
        }
        // No in-memory `active_theme` swap here — preview already left
        // this window showing `entry`, and every other window picks up
        // the change through the watcher's `ConfigEvent::Settings`
        // echo on the next config-poll tick.
    }

    /// Reload-driven apply: resolve `[ui].theme_dark` / `theme_light`
    /// against the bundled set and the user's themes directory, and
    /// swap the matching slot when the name differs from what's
    /// currently installed. Called once at startup from
    /// `apply_settings` (so the user's saved theme is honored on
    /// launch) and on every settings hot-reload (so a commit from one
    /// window applies to every other open window).
    pub(crate) fn apply_settings_theme_bindings(&mut self, s: &Settings) {
        let themes_dir = self.live_reload.as_ref().map(|r| r.themes_dir.clone());
        let mut next = self.active_theme.set.clone();
        let mut emitted: Vec<(ThemeSlot, String)> = Vec::new();

        let dark_target = s.ui.theme_dark.trim();
        if !dark_target.is_empty() && next.dark.name != dark_target {
            if let Some(theme) = resolve_theme_by_name(themes_dir.as_deref(), dark_target) {
                next.dark = theme;
                emitted.push((ThemeSlot::Dark, dark_target.to_string()));
            }
        }
        let light_target = s.ui.theme_light.trim();
        if !light_target.is_empty() && next.light.name != light_target {
            if let Some(theme) = resolve_theme_by_name(themes_dir.as_deref(), light_target) {
                next.light = theme;
                emitted.push((ThemeSlot::Light, light_target.to_string()));
            }
        }
        if emitted.is_empty() {
            return;
        }
        self.active_theme.set_installed(next);
        // Multi-window edge case: a picker open in this window stashed
        // the pre-picker `ThemeSet` as `original_set` for Esc to revert
        // to. A commit from a sibling window should become the new
        // revert target so cancel doesn't bounce back to a stale
        // pre-picker theme. Updating `original_set` here keeps the
        // picker's preview alive (the highlight + filter survive) while
        // making the cancel path honor the most recently committed
        // state.
        let new_original = self.active_theme.set.clone();
        if let crate::overlays::Overlays::ThemePicker(picker) = &mut self.overlays {
            picker.original_set = new_original;
        }
        for (slot, name) in emitted {
            crate::paint_trace::log_event(
                "theme_commit",
                &format!(
                    "source=settings_reload name={name} slot={} persisted=true",
                    slot_label(slot),
                ),
            );
        }
    }
}

/// Which slot the currently-resolved mode + OS-dark flag select.
/// Bundled into a helper so the picker / commit / settings-edit paths
/// can't drift on the resolution rule.
pub(crate) fn active_theme_slot(active: &crate::window_theme::ActiveTheme) -> ThemeSlot {
    let resolves_to_dark = matches!(active.mode, continuity_theme::Mode::Dark)
        || (matches!(active.mode, continuity_theme::Mode::System) && active.system_dark);
    if resolves_to_dark {
        ThemeSlot::Dark
    } else {
        ThemeSlot::Light
    }
}

fn slot_label(slot: ThemeSlot) -> &'static str {
    match slot {
        ThemeSlot::Dark => "dark",
        ThemeSlot::Light => "light",
    }
}

fn load_theme_for_entry(entry: &ThemeEntry) -> Option<Theme> {
    match &entry.source {
        crate::theme_picker::ThemeSource::Bundled => {
            continuity_theme::assets::bundled_named(&entry.name).ok()
        }
        crate::theme_picker::ThemeSource::UserFile(path) => std::fs::read_to_string(path)
            .ok()
            .and_then(|t| Theme::load(&t).ok()),
    }
}

/// Resolve a theme by user-facing name. Bundled names win first; falls
/// back to `<themes_dir>/<name>.toml`. Returns `None` when the name
/// matches nothing on disk and isn't bundled — apply_settings then
/// leaves the slot alone, which preserves the prior valid theme rather
/// than blanking it.
fn resolve_theme_by_name(themes_dir: Option<&std::path::Path>, name: &str) -> Option<Theme> {
    if continuity_theme::is_reserved_name(name) {
        return continuity_theme::assets::bundled_named(name).ok();
    }
    let dir = themes_dir?;
    let path = dir.join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path).ok()?;
    Theme::load(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_picker::{ThemeEntry, ThemeSource};
    use continuity_theme::{assets::bundled_set, Mode};

    #[test]
    fn active_slot_resolves_dark_mode_to_dark_slot() {
        let mut active = crate::window_theme::ActiveTheme::bundled().unwrap();
        active.set_mode(Mode::Dark);
        assert!(matches!(active_theme_slot(&active), ThemeSlot::Dark));
    }

    #[test]
    fn active_slot_resolves_light_mode_to_light_slot() {
        let mut active = crate::window_theme::ActiveTheme::bundled().unwrap();
        active.set_mode(Mode::Light);
        assert!(matches!(active_theme_slot(&active), ThemeSlot::Light));
    }

    #[test]
    fn active_slot_resolves_system_mode_via_os_flag() {
        let mut active = crate::window_theme::ActiveTheme::bundled().unwrap();
        active.set_mode(Mode::System);
        active.system_dark = true;
        active.set_mode(Mode::System);
        assert!(matches!(active_theme_slot(&active), ThemeSlot::Dark));
        active.system_dark = false;
        assert!(matches!(active_theme_slot(&active), ThemeSlot::Light));
    }

    #[test]
    fn resolve_theme_by_name_finds_bundled() {
        let t = resolve_theme_by_name(None, "deep_minimal");
        assert!(t.is_some());
        assert_eq!(t.unwrap().name, "deep_minimal");
    }

    #[test]
    fn resolve_theme_by_name_reads_user_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Build a minimal valid TOML by re-serializing a bundled theme.
        let mut bundled = continuity_theme::assets::bundled_named("deep_minimal").unwrap();
        bundled.name = "my_custom".to_string();
        let toml = bundled.to_toml();
        std::fs::write(tmp.path().join("my_custom.toml"), toml).unwrap();
        let t = resolve_theme_by_name(Some(tmp.path()), "my_custom");
        assert!(t.is_some());
        assert_eq!(t.unwrap().name, "my_custom");
    }

    #[test]
    fn resolve_theme_by_name_returns_none_for_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let t = resolve_theme_by_name(Some(tmp.path()), "no_such_theme");
        assert!(t.is_none());
    }

    #[test]
    fn load_theme_for_entry_handles_bundled() {
        let entry = ThemeEntry {
            name: "paper".to_string(),
            source: ThemeSource::Bundled,
        };
        let t = load_theme_for_entry(&entry);
        assert!(t.is_some());
    }

    #[test]
    fn load_theme_for_entry_returns_none_for_missing_user_file() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = ThemeEntry {
            name: "ghost".to_string(),
            source: ThemeSource::UserFile(tmp.path().join("ghost.toml")),
        };
        assert!(load_theme_for_entry(&entry).is_none());
    }

    #[test]
    fn slot_label_round_trips_through_settings_key() {
        // `ThemeSlot::settings_key` maps Dark → "theme_dark"; our
        // `slot_label` only needs to disambiguate in the trace event,
        // so we want "dark" / "light" without the prefix.
        assert_eq!(slot_label(ThemeSlot::Dark), "dark");
        assert_eq!(slot_label(ThemeSlot::Light), "light");
    }

    // Mirror of bundled_set() so the test relies on the same canary
    // path the production code does — keeps the `unused` warning off
    // when the rest of the file's test cluster doesn't reach for it.
    #[test]
    fn bundled_set_is_canary() {
        assert!(bundled_set().is_ok());
    }
}
