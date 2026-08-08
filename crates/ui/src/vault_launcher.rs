//! Fuzzy launcher state for machine-local known vaults.

use continuity_persist::KnownVault;
use continuity_search::{score, FuzzyMatch};

use crate::text_input::TextInput;

/// UI-thread state for the compact vault launcher.
#[derive(Debug, Default)]
pub struct VaultLauncher {
    /// Search input.
    pub input: TextInput,
    /// Full pinned/recent candidate set.
    pub all: Vec<KnownVault>,
    /// Indices into `all`, in fuzzy score/order.
    pub filtered: Vec<usize>,
    /// Match metadata parallel to `filtered`.
    pub matches: Vec<FuzzyMatch>,
    /// Selected filtered row.
    pub selected: usize,
}

/// Non-history actions appended to the launcher rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VaultLauncherAction {
    /// Choose any folder, opening it as a vault when initialized.
    Browse,
    /// Choose a folder and create its `.continuity` configuration.
    Initialize,
}

impl VaultLauncher {
    /// Construct a launcher populated with known vaults.
    #[must_use]
    pub fn new(all: Vec<KnownVault>) -> Self {
        let mut launcher = Self {
            all,
            ..Self::default()
        };
        launcher.refilter();
        launcher
    }

    /// Re-rank names and paths against the current query.
    pub(crate) fn refilter(&mut self) {
        let query = self.input.text.as_str();
        let mut scored = Vec::new();
        for (index, vault) in self.all.iter().enumerate() {
            let name = score(query, &vault.display_name);
            let path_text = vault.root_path.to_string_lossy();
            let path = score(query, &path_text);
            if let Some(mut matched) = name.or(path) {
                if vault.pinned {
                    matched.score += 1_000;
                }
                scored.push((index, matched));
            }
        }
        scored.sort_by(|(left_index, left), (right_index, right)| {
            right.score.cmp(&left.score).then_with(|| {
                self.all[*right_index]
                    .last_opened_ms
                    .cmp(&self.all[*left_index].last_opened_ms)
            })
        });
        self.filtered = scored.iter().map(|(index, _)| *index).collect();
        self.matches = scored.into_iter().map(|(_, matched)| matched).collect();
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
    }

    /// Move the selected row, clamped to the candidate list.
    pub(crate) fn step(&mut self, delta: i32) {
        self.selected =
            (self.selected as i32 + delta).clamp(0, self.row_count() as i32 - 1) as usize;
    }

    /// Currently selected vault.
    #[must_use]
    pub fn selected_vault(&self) -> Option<&KnownVault> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.all.get(*index))
    }

    /// Selected appended action, if the cursor is below the vault rows.
    #[must_use]
    pub fn selected_action(&self) -> Option<VaultLauncherAction> {
        match self.selected.checked_sub(self.filtered.len())? {
            0 => Some(VaultLauncherAction::Browse),
            1 => Some(VaultLauncherAction::Initialize),
            _ => None,
        }
    }

    /// Total selectable rows, including Browse and Initialize.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.filtered.len() + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn vault(name: &str, pinned: bool, recent: i64) -> KnownVault {
        KnownVault {
            root_path: PathBuf::from(format!(r"C:\notes\{name}")),
            display_name: name.into(),
            pinned,
            last_opened_ms: recent,
        }
    }

    #[test]
    fn pinned_vaults_rank_before_unpinned_empty_query() {
        let launcher =
            VaultLauncher::new(vec![vault("recent", false, 20), vault("pinned", true, 1)]);
        assert_eq!(launcher.selected_vault().unwrap().display_name, "pinned");
    }

    #[test]
    fn query_matches_path_as_well_as_name() {
        let mut launcher = VaultLauncher::new(vec![vault("work", false, 1)]);
        launcher.input.set_text("notes");
        launcher.refilter();
        assert_eq!(launcher.filtered.len(), 1);
    }

    #[test]
    fn browse_and_initialize_are_selectable_without_known_vaults() {
        let mut launcher = VaultLauncher::new(Vec::new());
        assert_eq!(
            launcher.selected_action(),
            Some(VaultLauncherAction::Browse)
        );
        launcher.step(1);
        assert_eq!(
            launcher.selected_action(),
            Some(VaultLauncherAction::Initialize)
        );
    }
}
