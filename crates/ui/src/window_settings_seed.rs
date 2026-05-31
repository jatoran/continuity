//! Seed + one-shot migration for `settings.toml`.
//!
//! VSCode / Sublime model: the user file holds **overrides only**.
//! Defaults live in the binary and are discoverable through the
//! command palette and `.docs/generated/SETTINGS.md`. The earlier
//! continuity design pasted every default into the file as a
//! commented cheatsheet so new users could see what was available;
//! that turned out to create visual redundancy once writebacks
//! started appending live overrides above the cheatsheet
//! ([editor].font_family_prose ended up in two places at once —
//! once as a real value, once as the dead `# font_family_prose = …`
//! line below).
//!
//! This module owns:
//!
//! - [`SETTINGS_TEMPLATE`] — the minimal header seeded on first run.
//! - [`ensure_settings_file`] — creates the file if missing and
//!   quietly trims the legacy cheatsheet from existing files.
//! - [`strip_legacy_cheatsheet`] — pure function exposed for tests.
//!
//! Thread ownership: called from the UI thread during settings.open
//! resolution, and indirectly via the live-reload startup path.

use std::path::Path;

/// Minimal seed for a brand-new `settings.toml`. Holds nothing but a
/// short pointer to where defaults live; the file fills up as the
/// user toggles knobs and the writeback helpers append entries.
pub(crate) const SETTINGS_TEMPLATE: &str = "\
# continuity user settings — overrides only.
# Defaults live in the binary; this file holds your changes.
# Hot-reloaded on save. See `.docs/generated/SETTINGS.md` for every
# available key, or run `settings.open` (Ctrl+,) any time.
";

/// Distinctive line in the legacy chatty `SETTINGS_TEMPLATE`. Used by
/// [`strip_legacy_cheatsheet`] as a single, stable signature so the
/// migration runs only on files that were seeded by the old code
/// path. Matched as a full line (exact byte equality), so a user
/// comment that happens to include the substring does not trigger a
/// false strip.
const LEGACY_TEMPLATE_MARKER: &str =
    "# This file is hot-reloaded: edits land the next time the editor reads it.";

/// Trim the legacy commented-default cheatsheet from a settings.toml
/// body when present.
///
/// Detection: a line equal (byte-for-byte) to
/// [`LEGACY_TEMPLATE_MARKER`]. On match, everything from that line —
/// and the contiguous block of comment / blank lines immediately
/// above it (the "# continuity — user settings" header) — through
/// the end of file is dropped. The legacy template was always
/// appended at the *bottom* of the file by the original
/// `ensure_settings_file`, and live overrides live above any seeded
/// text because `toml_edit` writes new tables in document order, not
/// at EOF. So trimming tailward is safe.
///
/// Returns `(trimmed_body, changed)` so the caller can decide whether
/// to rewrite the file; an unchanged body is returned verbatim.
pub(crate) fn strip_legacy_cheatsheet(body: &str) -> (String, bool) {
    let Some(marker_byte) = body
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1; // +1 for the '\n' the iterator dropped
            Some((start, line))
        })
        .find_map(|(start, line)| (line == LEGACY_TEMPLATE_MARKER).then_some(start))
    else {
        return (body.to_string(), false);
    };
    let mut cut = marker_byte;
    while cut > 0 {
        let prev_newline = body[..cut - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line = &body[prev_newline..cut - 1];
        if line.starts_with('#') || line.is_empty() {
            cut = prev_newline;
        } else {
            break;
        }
    }
    let head = body[..cut].trim_end().to_string();
    let trimmed = if head.is_empty() {
        String::new()
    } else {
        let mut h = head;
        h.push('\n');
        h
    };
    debug_assert!(!trimmed.contains(LEGACY_TEMPLATE_MARKER));
    (trimmed, true)
}

/// Seed `settings.toml` on first run and apply the one-shot legacy
/// strip on existing files.
pub(crate) fn ensure_settings_file(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        if let Ok(body) = std::fs::read_to_string(path) {
            let (trimmed, changed) = strip_legacy_cheatsheet(&body);
            if changed {
                std::fs::write(path, trimmed)?;
            }
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, SETTINGS_TEMPLATE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Snapshot of the pre-VSCode-style `SETTINGS_TEMPLATE`. Kept
    /// inline so the strip tests don't drift if the live constant
    /// changes again.
    const LEGACY_TEMPLATE: &str = "\
# continuity — user settings
#
# This file is hot-reloaded: edits land the next time the editor reads it.
# Every key below is the in-binary default; uncomment + edit to override.

# [editor]
# font_family_prose = \"Segoe UI Variable\"
# font_size = 14

# [ui]
# theme = \"system\"
";

    #[test]
    fn strip_is_noop_on_minimal_seed() {
        let (out, changed) = strip_legacy_cheatsheet(SETTINGS_TEMPLATE);
        assert!(!changed);
        assert_eq!(out, SETTINGS_TEMPLATE);
    }

    #[test]
    fn strip_drops_legacy_cheatsheet_entirely_when_file_only_contains_it() {
        let (out, changed) = strip_legacy_cheatsheet(LEGACY_TEMPLATE);
        assert!(changed);
        assert!(out.is_empty(), "expected empty body, got: {out:?}");
    }

    #[test]
    fn strip_preserves_live_overrides_above_cheatsheet() {
        let mut input = String::from("[editor]\nfont_family_prose = \"Consolas\"\n\n");
        input.push_str(LEGACY_TEMPLATE);
        let (out, changed) = strip_legacy_cheatsheet(&input);
        assert!(changed);
        assert!(out.contains("font_family_prose = \"Consolas\""));
        assert!(!out.contains(LEGACY_TEMPLATE_MARKER));
        assert!(!out.contains("# font_family_prose = \"Segoe UI Variable\""));
    }

    #[test]
    fn strip_does_not_trigger_on_substring_in_user_comment() {
        let input =
            "# personal note: this file is hot-reloaded sometimes\n[editor]\nword_wrap = true\n";
        let (out, changed) = strip_legacy_cheatsheet(input);
        assert!(!changed, "marker is anchored to full-line equality");
        assert_eq!(out, input);
    }

    #[test]
    fn ensure_settings_file_creates_minimal_seed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        ensure_settings_file(&path).unwrap();
        let read_back = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read_back, SETTINGS_TEMPLATE);
    }

    #[test]
    fn ensure_settings_file_strips_legacy_in_place() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut existing = String::from("[editor]\nfont_family_prose = \"Consolas\"\n\n");
        existing.push_str(LEGACY_TEMPLATE);
        std::fs::write(&path, &existing).unwrap();
        ensure_settings_file(&path).unwrap();
        let read_back = std::fs::read_to_string(&path).unwrap();
        assert!(read_back.contains("font_family_prose = \"Consolas\""));
        assert!(!read_back.contains(LEGACY_TEMPLATE_MARKER));
    }
}
