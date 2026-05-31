//! Theme-picker overlay (§E4).
//!
//! Lists every theme discoverable from disk (the user's themes directory)
//! plus the bundled `deep_minimal` and `paper`. Moving the highlight
//! re-renders the editor live in the highlighted theme; Enter keeps it
//! and Esc reverts to the theme set in effect when the palette opened.
//!
//! Discovery + apply / revert run on the UI thread of the window that
//! owns the overlay. The picker state itself is pure — it carries the
//! enumerated names and the throttle bookkeeping (one preview per
//! highlight change, never per filter-line keystroke).

use std::path::PathBuf;

use continuity_search::{score, FuzzyMatch};
use continuity_theme::ThemeSet;

use crate::text_input::TextInput;

/// Source of a theme entry: a bundled theme baked into the binary or a
/// user-installed TOML in the themes directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// Bundled (`crates/theme/assets/*.toml`, loadable via
    /// [`continuity_theme::assets`]).
    Bundled,
    /// User-installed file at the given path.
    UserFile(PathBuf),
}

/// One enumerated theme.
#[derive(Clone, Debug)]
pub struct ThemeEntry {
    /// Theme name (file stem or `Theme.name` field).
    pub name: String,
    /// Where the theme lives.
    pub source: ThemeSource,
}

/// Live state for an open theme picker.
#[derive(Debug)]
pub struct ThemePicker {
    /// Filter input shown at the top of the panel.
    pub input: TextInput,
    /// All discovered themes, name-sorted case-insensitively.
    pub all: Vec<ThemeEntry>,
    /// Indices into `all` of the currently-shown matches, in score order.
    pub filtered: Vec<usize>,
    /// Per-filtered-row fuzzy-match metadata.
    pub matches: Vec<FuzzyMatch>,
    /// Selected row within `filtered`.
    pub selected: usize,
    /// `ThemeSet` in effect when the picker opened — Esc restores this.
    pub original_set: ThemeSet,
    /// Theme name of the currently-active mode at open time (used to
    /// anchor the highlight on the matching row).
    pub original_name: String,
    /// Last preview applied, to avoid redundant work when `refilter`
    /// shuffles the row order but keeps the highlight on the same name.
    pub last_previewed: Option<String>,
}

impl ThemePicker {
    /// Open a picker preloaded with `all` discovered themes. `original_set`
    /// is the set Esc should restore; `original_name` is used to anchor
    /// the initial highlight on the row that matches the active theme.
    #[must_use]
    pub fn new(all: Vec<ThemeEntry>, original_set: ThemeSet, original_name: String) -> Self {
        let mut picker = Self {
            input: TextInput::default(),
            all,
            filtered: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            original_set,
            original_name: original_name.clone(),
            last_previewed: None,
        };
        picker.refilter();
        if let Some(idx) = picker
            .filtered
            .iter()
            .position(|i| picker.all[*i].name == original_name)
        {
            picker.selected = idx;
        }
        picker.last_previewed = Some(original_name);
        picker
    }

    /// Re-rank `all` against the current filter line. Preserves the
    /// selected theme by name when possible (so a row that survives the
    /// new filter keeps the highlight); otherwise falls back to the
    /// top-scoring match.
    pub(crate) fn refilter(&mut self) {
        let q = self.input.text.as_str();
        let prev_name = self
            .filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
            .map(|e| e.name.clone());
        let mut scored: Vec<(usize, FuzzyMatch)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, e)| score(q, &e.name).map(|m| (i, m)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.score.cmp(&a.1.score).then_with(|| {
                self.all[a.0]
                    .name
                    .to_ascii_lowercase()
                    .cmp(&self.all[b.0].name.to_ascii_lowercase())
            })
        });
        self.filtered.clear();
        self.matches.clear();
        for (i, m) in scored {
            self.filtered.push(i);
            self.matches.push(m);
        }
        if let Some(name) = prev_name {
            if let Some(pos) = self.filtered.iter().position(|i| self.all[*i].name == name) {
                self.selected = pos;
                return;
            }
        }
        self.selected = 0;
    }

    /// Move the highlight by `delta`, clamping.
    pub fn step(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as i32;
        let next = (self.selected as i32 + delta).max(0).min(len - 1);
        self.selected = next as usize;
    }

    /// The currently-highlighted entry, if any.
    #[must_use]
    pub(crate) fn selected_entry(&self) -> Option<&ThemeEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.all.get(*i))
    }

    /// Return `Some(entry)` for the caller to apply when the highlighted
    /// theme has changed since the last preview, otherwise `None`. The
    /// caller — the Window — does the load + slot-swap.
    pub(crate) fn next_preview(&mut self) -> Option<&ThemeEntry> {
        let candidate_name = self.selected_entry()?.name.clone();
        if self.last_previewed.as_deref() == Some(candidate_name.as_str()) {
            return None;
        }
        self.last_previewed = Some(candidate_name);
        self.selected_entry()
    }

    /// ThemeSet to restore on Esc.
    #[must_use]
    pub(crate) fn revert_set(&self) -> ThemeSet {
        self.original_set.clone()
    }
}

/// Enumerate available themes. Bundled themes are always included; user
/// themes are listed when `themes_dir` is `Some` and the directory is
/// readable. Names are deduplicated by case-insensitive match, with the
/// user-file entry winning so a user can override a bundled name.
#[must_use]
pub(crate) fn enumerate_themes(themes_dir: Option<&std::path::Path>) -> Vec<ThemeEntry> {
    let mut out: Vec<ThemeEntry> = continuity_theme::assets::BUNDLED_NAMES
        .iter()
        .map(|name| ThemeEntry {
            name: (*name).to_string(),
            source: ThemeSource::Bundled,
        })
        .collect();
    if let Some(dir) = themes_dir {
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                out.push(ThemeEntry {
                    name: stem.to_string(),
                    source: ThemeSource::UserFile(path),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    // User-file entries that share a name with a bundled one override the
    // bundled — keep the first occurrence (sorted lowercase, but user-file
    // sources tie-break later than bundled in iteration order; force the
    // user-file to win by sweeping after dedup).
    let mut by_name: std::collections::BTreeMap<String, ThemeEntry> =
        std::collections::BTreeMap::new();
    for entry in out {
        let key = entry.name.to_ascii_lowercase();
        match by_name.get(&key) {
            None => {
                by_name.insert(key, entry);
            }
            Some(existing) => {
                if matches!(existing.source, ThemeSource::Bundled)
                    && matches!(entry.source, ThemeSource::UserFile(_))
                {
                    by_name.insert(key, entry);
                }
            }
        }
    }
    by_name.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use continuity_theme::assets::bundled_set;

    fn entries() -> Vec<ThemeEntry> {
        vec![
            ThemeEntry {
                name: "deep_minimal".into(),
                source: ThemeSource::Bundled,
            },
            ThemeEntry {
                name: "paper".into(),
                source: ThemeSource::Bundled,
            },
            ThemeEntry {
                name: "solarized_dark".into(),
                source: ThemeSource::Bundled,
            },
        ]
    }

    #[test]
    fn new_anchors_on_original_name() {
        let p = ThemePicker::new(entries(), bundled_set().unwrap(), "paper".into());
        assert_eq!(p.selected_entry().unwrap().name, "paper");
    }

    #[test]
    fn unknown_original_anchors_to_first_row() {
        let p = ThemePicker::new(entries(), bundled_set().unwrap(), "not_installed".into());
        assert_eq!(p.selected_entry().unwrap().name, "deep_minimal");
    }

    #[test]
    fn refilter_holds_by_name_when_possible() {
        let mut p = ThemePicker::new(entries(), bundled_set().unwrap(), "paper".into());
        p.input.set_text("sol");
        p.refilter();
        assert_eq!(p.selected_entry().unwrap().name, "solarized_dark");
    }

    #[test]
    fn step_clamps_to_bounds() {
        let mut p = ThemePicker::new(entries(), bundled_set().unwrap(), "paper".into());
        p.step(-20);
        assert!(matches!(p.selected, 0));
        p.step(20);
        assert_eq!(p.selected_entry().unwrap().name, "solarized_dark");
    }

    #[test]
    fn next_preview_throttles_per_highlight() {
        let mut p = ThemePicker::new(entries(), bundled_set().unwrap(), "paper".into());
        // open with paper highlighted, last_previewed == paper → None.
        assert!(p.next_preview().is_none());
        p.step(1);
        let v = p.next_preview();
        assert!(v.is_some());
        // No step → no more preview.
        assert!(p.next_preview().is_none());
    }

    #[test]
    fn revert_set_clones_original() {
        let set = bundled_set().unwrap();
        let p = ThemePicker::new(entries(), set.clone(), "paper".into());
        let r = p.revert_set();
        assert_eq!(r.dark.name, set.dark.name);
        assert_eq!(r.light.name, set.light.name);
    }

    #[test]
    fn enumerate_themes_without_dir_returns_bundled_only() {
        let v = enumerate_themes(None);
        let names: Vec<_> = v.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"deep_minimal".to_string()));
        assert!(names.contains(&"paper".to_string()));
    }

    #[test]
    fn enumerate_themes_picks_up_user_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("custom.toml"), "name = \"custom\"\n").unwrap();
        std::fs::write(tmp.path().join("not_a_theme.txt"), "ignore me").unwrap();
        let v = enumerate_themes(Some(tmp.path()));
        let names: Vec<_> = v.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"custom".to_string()));
        assert!(!names.iter().any(|n| n == "not_a_theme"));
    }

    #[test]
    fn enumerate_themes_user_file_overrides_bundled_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("paper.toml"), "name = \"paper\"\n").unwrap();
        let v = enumerate_themes(Some(tmp.path()));
        let paper = v.iter().find(|e| e.name == "paper").unwrap();
        assert!(matches!(paper.source, ThemeSource::UserFile(_)));
    }
}
