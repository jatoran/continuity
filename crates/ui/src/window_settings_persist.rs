//! δ.6 Tier 3 — bidirectional sync helper for boolean toggle commands
//! and scalar commit commands (font family / size).
//!
//! Contract (C) from `.docs/design/defaults.md` "Hot-reload contract":
//! when a toggle command flips a runtime boolean, or a scalar commit
//! command picks a new value (font family / size), the change is also
//! written back to `settings.toml` so it is durable across relaunch.
//! The file is always the source of truth; the runtime value is its
//! mirror.
//!
//! Thread ownership: every entry point on [`crate::Window`] is called
//! from the UI thread. `toml_edit` preserves comments + key ordering of
//! the on-disk file so a writeback round-trip does not churn the user's
//! manual edits.
//!
//! Ping-pong protection: the writer increments
//! [`crate::window_settings_projections::SettingsProjections::writeback_in_flight`]
//! before writing. The next inbound
//! [`continuity_config::ConfigEvent::Settings`] decrements the counter
//! and skips `apply_settings`, so our own writeback does not cause a
//! redundant projection round trip.

use std::fs;
use std::io::Write;
use std::path::Path;

use toml_edit::{value, DocumentMut, Item, Table, Value};

use crate::Window;

/// Error type local to the persist helper. Mapped to
/// [`crate::Error::Command`] at the call site so it can be surfaced as
/// a non-blocking banner. Kept as a `String`-bearing transparent variant
/// rather than a `thiserror` enum because every consumer turns it
/// straight into a user-facing message.
#[derive(Debug)]
pub(crate) struct PersistError(pub String);

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Window {
    /// Persist a boolean setting back to `settings.toml`.
    ///
    /// Thin wrapper over [`Window::persist_scalar_setting`] —
    /// see that helper for the full IO + echo-suppression contract.
    pub(crate) fn persist_boolean_setting(
        &mut self,
        section: &str,
        key: &str,
        value_bool: bool,
    ) -> Result<(), PersistError> {
        self.persist_scalar_setting(section, key, Value::from(value_bool))
    }

    /// Persist a string setting (font family, theme name, etc.) back
    /// to `settings.toml`. See [`Window::persist_scalar_setting`].
    pub(crate) fn persist_string_setting(
        &mut self,
        section: &str,
        key: &str,
        value_str: &str,
    ) -> Result<(), PersistError> {
        self.persist_scalar_setting(section, key, Value::from(value_str))
    }

    /// Persist a floating-point setting (font size, line height,
    /// etc.) back to `settings.toml`. See
    /// [`Window::persist_scalar_setting`].
    pub(crate) fn persist_float_setting(
        &mut self,
        section: &str,
        key: &str,
        value_f64: f64,
    ) -> Result<(), PersistError> {
        self.persist_scalar_setting(section, key, Value::from(value_f64))
    }

    /// Core writeback path shared by every scalar persistence helper.
    ///
    /// Reads the file (or the bundled template if the file does not
    /// exist yet), parses with `toml_edit` to preserve comments +
    /// ordering, sets `[section].key = value`, and writes atomically
    /// via a temp-file + rename. Increments
    /// [`SettingsProjections::writeback_in_flight`](crate::window_settings_projections::SettingsProjections::writeback_in_flight)
    /// so the next watcher event is recognised as our own echo.
    ///
    /// Returns [`PersistError`] if the path is unavailable, the file
    /// is unreadable, the TOML is malformed, or the write fails.
    fn persist_scalar_setting(
        &mut self,
        section: &str,
        key: &str,
        value_item: Value,
    ) -> Result<(), PersistError> {
        let Some(reload) = self.live_reload.as_ref() else {
            return Err(PersistError(
                "settings persist: live-reload unavailable (test stub or early init)".into(),
            ));
        };
        let path = reload.settings_path.clone();
        let original = read_or_empty(&path)?;
        let mut doc: DocumentMut = if original.trim().is_empty() {
            DocumentMut::new()
        } else {
            original
                .parse::<DocumentMut>()
                .map_err(|e| PersistError(format!("settings.toml: parse failed: {e}")))?
        };
        ensure_table(&mut doc, section);
        doc[section][key] = value(value_item);
        let rendered = doc.to_string();
        // Idempotency: don't bump the in-flight counter or touch disk
        // if the file already matches what we'd write. Avoids spurious
        // watcher events on a redundant commit (e.g. confirming the
        // font picker without changing the highlighted family).
        if rendered == original {
            return Ok(());
        }
        atomic_write(&path, rendered.as_bytes())?;
        self.settings_projections.writeback_in_flight = self
            .settings_projections
            .writeback_in_flight
            .saturating_add(1);
        Ok(())
    }

    /// Flip a boolean: persist the negation of `current`, returning the
    /// new value. The runtime field is the caller's responsibility — the
    /// helper writes the file half of contract (C); the toggle command
    /// owns the runtime half before calling.
    ///
    /// Currently unused — staged for the δ.6 Tier 4 gap-fill commands
    /// (per-key toggle wrappers) that haven't landed yet; remove or
    /// inline once those commands ship.
    #[allow(dead_code)]
    pub(crate) fn toggle_boolean_setting(
        &mut self,
        section: &str,
        key: &str,
        current: bool,
    ) -> Result<bool, PersistError> {
        let new_value = !current;
        self.persist_boolean_setting(section, key, new_value)?;
        Ok(new_value)
    }

    /// Convenience wrapper: persist a boolean and log on failure rather
    /// than propagate. Used by toggle command implementations where the
    /// runtime field has already been flipped and a persistence failure
    /// is a soft error — the toggle works for the session but won't
    /// survive relaunch. Failure is rare (file-system error) and visible
    /// in stderr so it isn't silently lost.
    pub(crate) fn persist_toggle_or_log(&mut self, section: &str, key: &str, value_bool: bool) {
        if let Err(e) = self.persist_boolean_setting(section, key, value_bool) {
            eprintln!("continuity: persist {section}.{key} failed: {e}");
        }
    }

    /// Soft-failure companion to [`Window::persist_string_setting`].
    /// Used by scalar commit commands (font family, theme name) where
    /// the runtime value has already been applied and a persistence
    /// failure should not block the user — they get the change for
    /// this session, and a stderr line records why it didn't survive.
    pub(crate) fn persist_string_or_log(&mut self, section: &str, key: &str, value_str: &str) {
        if let Err(e) = self.persist_string_setting(section, key, value_str) {
            eprintln!("continuity: persist {section}.{key} failed: {e}");
        }
    }

    /// Soft-failure companion to [`Window::persist_float_setting`].
    /// Used by scalar commit commands (font size, line height) — same
    /// contract as [`Window::persist_string_or_log`].
    pub(crate) fn persist_float_or_log(&mut self, section: &str, key: &str, value_f64: f64) {
        if let Err(e) = self.persist_float_setting(section, key, value_f64) {
            eprintln!("continuity: persist {section}.{key} failed: {e}");
        }
    }

    /// Decrement the writeback counter. Called by
    /// [`crate::Window::handle_config_event`] when a
    /// [`continuity_config::ConfigEvent::Settings`] arrives with the
    /// counter non-zero — the event is treated as our own echo and
    /// `apply_settings` is skipped. Returns `true` if the event should
    /// be skipped, `false` if it should proceed normally.
    pub(crate) fn consume_writeback_echo(&mut self) -> bool {
        if self.settings_projections.writeback_in_flight == 0 {
            return false;
        }
        self.settings_projections.writeback_in_flight -= 1;
        true
    }
}

fn read_or_empty(path: &Path) -> Result<String, PersistError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(PersistError(format!(
            "settings.toml: read failed: {} ({e})",
            path.display()
        ))),
    }
}

fn ensure_table(doc: &mut DocumentMut, section: &str) {
    if doc.get(section).is_none() {
        doc[section] = Item::Table(Table::new());
    } else if !doc[section].is_table() {
        // Replace a malformed non-table entry rather than panic. The
        // user's previous value is lost, but the file remains parseable.
        doc[section] = Item::Table(Table::new());
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PersistError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            PersistError(format!(
                "settings.toml: create parent {} failed: {e}",
                parent.display()
            ))
        })?;
    }
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| {
            PersistError(format!(
                "settings.toml: create temp {} failed: {e}",
                tmp.display()
            ))
        })?;
        f.write_all(bytes).map_err(|e| {
            PersistError(format!(
                "settings.toml: write temp {} failed: {e}",
                tmp.display()
            ))
        })?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).map_err(|e| {
        PersistError(format!(
            "settings.toml: rename {} -> {} failed: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn parse(text: &str) -> DocumentMut {
        text.parse::<DocumentMut>().expect("test toml parses")
    }

    #[test]
    fn ensure_table_creates_missing_section() {
        let mut doc = parse("[editor]\nfont_size = 14\n");
        ensure_table(&mut doc, "ui");
        assert!(doc["ui"].is_table());
    }

    #[test]
    fn ensure_table_replaces_non_table_entry() {
        let mut doc = parse("ui = 1\n");
        ensure_table(&mut doc, "ui");
        assert!(doc["ui"].is_table());
    }

    #[test]
    fn atomic_write_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        atomic_write(&path, b"hello = true\n").unwrap();
        let read_back = fs::read_to_string(&path).unwrap();
        assert_eq!(read_back, "hello = true\n");
    }

    #[test]
    fn writing_to_empty_file_creates_section_and_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        // Simulate the helper's body without needing a Window.
        let original = read_or_empty(&path).unwrap();
        let mut doc: DocumentMut = if original.trim().is_empty() {
            DocumentMut::new()
        } else {
            original.parse().unwrap()
        };
        ensure_table(&mut doc, "ui");
        doc["ui"]["show_outline_sidebar"] = value(true);
        atomic_write(&path, doc.to_string().as_bytes()).unwrap();

        let read_back = fs::read_to_string(&path).unwrap();
        assert!(read_back.contains("[ui]"));
        assert!(read_back.contains("show_outline_sidebar = true"));
    }

    #[test]
    fn writing_preserves_unrelated_comments_and_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let original = "# top comment\n[editor]\n# editor comment\nfont_size = 14\n\n[ui]\nshow_minimap = false\n";
        fs::write(&path, original).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let mut doc: DocumentMut = text.parse().unwrap();
        ensure_table(&mut doc, "ui");
        doc["ui"]["show_minimap"] = value(true);
        atomic_write(&path, doc.to_string().as_bytes()).unwrap();

        let read_back = fs::read_to_string(&path).unwrap();
        assert!(read_back.contains("# top comment"));
        assert!(read_back.contains("# editor comment"));
        assert!(read_back.contains("font_size = 14"));
        assert!(read_back.contains("show_minimap = true"));
    }

    #[test]
    fn idempotent_rewrite_produces_byte_identical_output() {
        // The persist helper short-circuits when the rendered output
        // matches the on-disk text; this test asserts the round-trip
        // through `toml_edit` is byte-stable for an unchanged value.
        let original = "[ui]\nshow_minimap = true\n";
        let mut doc: DocumentMut = original.parse().unwrap();
        ensure_table(&mut doc, "ui");
        doc["ui"]["show_minimap"] = value(true);
        let rendered = doc.to_string();
        assert_eq!(rendered, original);
    }

    #[test]
    fn string_writeback_round_trips_value_and_preserves_comments() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let original = "# user-edited\n[editor]\n# font_family_prose = \"Segoe UI Variable\"\nfont_size = 14\n";
        fs::write(&path, original).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let mut doc: DocumentMut = text.parse().unwrap();
        ensure_table(&mut doc, "editor");
        doc["editor"]["font_family_prose"] = value(Value::from("Consolas"));
        atomic_write(&path, doc.to_string().as_bytes()).unwrap();

        let read_back = fs::read_to_string(&path).unwrap();
        assert!(read_back.contains("# user-edited"));
        assert!(read_back.contains("font_size = 14"));
        assert!(read_back.contains("font_family_prose = \"Consolas\""));
    }

    #[test]
    fn float_writeback_renders_decimal_literal() {
        let mut doc: DocumentMut = "[editor]\n".parse().unwrap();
        ensure_table(&mut doc, "editor");
        doc["editor"]["font_size"] = value(Value::from(15.5_f64));
        let rendered = doc.to_string();
        assert!(rendered.contains("font_size = 15.5"));
    }

    #[test]
    fn idempotent_string_rewrite_is_byte_stable() {
        let original = "[editor]\nfont_family_prose = \"Consolas\"\n";
        let mut doc: DocumentMut = original.parse().unwrap();
        ensure_table(&mut doc, "editor");
        doc["editor"]["font_family_prose"] = value(Value::from("Consolas"));
        assert_eq!(doc.to_string(), original);
    }
}
