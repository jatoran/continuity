//! UI-thread state for one window's optional vault workspace.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use continuity_buffer::BufferId;
use continuity_config::VaultConfig;

/// A pending debounced export for a vault-owned file buffer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingVaultAutosave {
    pub(crate) due_ms: u64,
    pub(crate) revision: u64,
}

/// In-flight restore of a vault's saved open tabs. Populated when a vault
/// with persisted `open_tabs` is opened; each entry is consumed as its file
/// finishes opening (so the tab's scroll can be seeded and the focused tab
/// activated). Cleared when every issued open has been accounted for.
#[derive(Clone, Debug, Default)]
pub(crate) struct PendingVaultTabRestore {
    /// Absolute file path → saved vertical scroll offset (DIPs).
    pub(crate) scroll_by_path: HashMap<PathBuf, f32>,
    /// Absolute path of the tab that should end up focused, if any.
    pub(crate) focused_path: Option<PathBuf>,
    /// Number of opens still expected (decremented on open success/failure).
    pub(crate) remaining: usize,
}

/// Vault state owned exclusively by one window's UI thread.
#[derive(Default)]
pub(crate) struct VaultState {
    root: Option<PathBuf>,
    config: Option<VaultConfig>,
    browse_root: Option<PathBuf>,
    pub(crate) pending_autosaves: HashMap<BufferId, PendingVaultAutosave>,
    /// Buffers with an autosave write dispatched but not yet acknowledged,
    /// mapped to the `now_ms` at dispatch. The timestamp backs a watchdog:
    /// if an acknowledgement is somehow lost, the entry is swept so the
    /// buffer can retry instead of wedging autosave permanently.
    pub(crate) autosaves_in_flight: HashMap<BufferId, u64>,
    pub(crate) suspended_autosaves: HashSet<BufferId>,
    pending_delete: Option<(PathBuf, u64)>,
    theme_base: Option<continuity_theme::Theme>,
    open_window: Option<crate::window_config::OpenVaultWindow>,
    activated: Option<crate::window_config::VaultActivated>,
    pub(crate) pending_tab_restore: Option<PendingVaultTabRestore>,
}

impl VaultState {
    pub(crate) fn with_callbacks(
        open_window: Option<crate::window_config::OpenVaultWindow>,
        activated: Option<crate::window_config::VaultActivated>,
    ) -> Self {
        Self {
            open_window,
            activated,
            ..Self::default()
        }
    }

    pub(crate) fn open_in_window(&self, root: PathBuf) -> bool {
        let Some(open_window) = self.open_window.as_ref() else {
            return false;
        };
        open_window(root);
        true
    }

    pub(crate) fn notify_activated(&self, root: PathBuf) {
        if let Some(activated) = self.activated.as_ref() {
            activated(root);
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.config.is_some()
    }

    pub(crate) fn theme_base(&self) -> Option<&continuity_theme::Theme> {
        self.theme_base.as_ref()
    }

    pub(crate) fn set_theme_base(&mut self, theme: continuity_theme::Theme) {
        self.theme_base = Some(theme);
    }

    pub(crate) fn take_theme_base(&mut self) -> Option<continuity_theme::Theme> {
        self.theme_base.take()
    }

    pub(crate) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub(crate) fn config(&self) -> Option<&VaultConfig> {
        self.config.as_ref()
    }

    pub(crate) fn browse_root(&self) -> Option<&Path> {
        self.browse_root.as_deref()
    }

    pub(crate) fn apply_inspection(
        &mut self,
        requested_root: PathBuf,
        root: PathBuf,
        config: Option<VaultConfig>,
    ) {
        self.root = config.as_ref().map(|_| root);
        self.config = config;
        self.browse_root = Some(requested_root);
        self.pending_autosaves.clear();
        self.autosaves_in_flight.clear();
        self.suspended_autosaves.clear();
        self.pending_delete = None;
        self.pending_tab_restore = None;
    }

    pub(crate) fn update_config(&mut self, root: &Path, config: VaultConfig) -> bool {
        if self.root.as_deref() != Some(root) {
            return false;
        }
        self.config = Some(config);
        true
    }

    pub(crate) fn confirm_delete(&mut self, relative: &Path, now_ms: u64) -> bool {
        if self
            .pending_delete
            .as_ref()
            .is_some_and(|(pending, armed_ms)| {
                pending == relative && now_ms.saturating_sub(*armed_ms) <= 3_000
            })
        {
            self.pending_delete = None;
            true
        } else {
            self.pending_delete = Some((relative.to_path_buf(), now_ms));
            false
        }
    }

    pub(crate) fn suspend_autosave(&mut self, buffer_id: BufferId) {
        self.pending_autosaves.remove(&buffer_id);
        self.autosaves_in_flight.remove(&buffer_id);
        self.suspended_autosaves.insert(buffer_id);
    }

    pub(crate) fn resume_autosave(&mut self, buffer_id: BufferId) {
        self.suspended_autosaves.remove(&buffer_id);
    }

    pub(crate) fn owns_file(&self, path: &Path) -> bool {
        let (Some(root), Some(config)) = (self.root.as_ref(), self.config.as_ref()) else {
            return false;
        };
        let Some(relative) = compute_relative_vault_path(root, path) else {
            return false;
        };
        !config.is_path_ignored(&relative)
    }

    /// The vault-relative, slash-normalized form of `path` when it is owned by
    /// this vault (under the root and not ignored). Used to record open tabs
    /// portably in `workspace.toml`.
    pub(crate) fn relative_path_for(&self, path: &Path) -> Option<String> {
        if !self.owns_file(path) {
            return None;
        }
        let root = self.root.as_ref()?;
        let relative = compute_relative_vault_path(root, path)?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }

    /// Resolve a vault-relative slash path to an absolute path under the root.
    pub(crate) fn absolute_path_for(&self, relative: &str) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        Some(root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)))
    }
}

fn compute_relative_vault_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let root_components: Vec<_> = root.components().collect();
    let path_components: Vec<_> = path.components().collect();
    if path_components.len() < root_components.len()
        || !root_components
            .iter()
            .zip(&path_components)
            .all(|(left, right)| {
                left.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
            })
    {
        return None;
    }
    Some(
        path_components[root_components.len()..]
            .iter()
            .map(|component| component.as_os_str())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_vault_owns_only_non_ignored_descendants() {
        let mut state = VaultState::default();
        let root = PathBuf::from(r"C:\notes");
        state.apply_inspection(root.clone(), root.clone(), Some(VaultConfig::default()));
        assert!(state.owns_file(&root.join("daily.md")));
        assert!(state.owns_file(Path::new(r"c:\NOTES\daily.md")));
        assert!(!state.owns_file(&root.join(".continuity").join("vault.toml")));
        assert!(!state.owns_file(Path::new(r"C:\notes-extra\daily.md")));
        assert!(!state.owns_file(Path::new(r"C:\elsewhere\daily.md")));
    }

    #[test]
    fn relative_and_absolute_paths_round_trip_for_owned_files() {
        let mut state = VaultState::default();
        let root = PathBuf::from(r"C:\notes");
        state.apply_inspection(root.clone(), root.clone(), Some(VaultConfig::default()));

        let relative = state
            .relative_path_for(&root.join("daily").join("today.md"))
            .expect("owned file has a relative path");
        assert_eq!(relative, "daily/today.md");
        assert_eq!(
            state.absolute_path_for(&relative),
            Some(root.join("daily").join("today.md")),
        );

        // Files outside the vault or in the config dir are not portable.
        assert_eq!(
            state.relative_path_for(Path::new(r"C:\elsewhere\a.md")),
            None
        );
        assert_eq!(
            state.relative_path_for(&root.join(".continuity").join("vault.toml")),
            None
        );
    }

    #[test]
    fn suspended_autosave_clears_pending_work_until_resumed() {
        let mut state = VaultState::default();
        let buffer_id = BufferId::new();
        state.pending_autosaves.insert(
            buffer_id,
            PendingVaultAutosave {
                due_ms: 10,
                revision: 2,
            },
        );
        state.autosaves_in_flight.insert(buffer_id, 0);
        state.suspend_autosave(buffer_id);
        assert!(!state.pending_autosaves.contains_key(&buffer_id));
        assert!(!state.autosaves_in_flight.contains_key(&buffer_id));
        assert!(state.suspended_autosaves.contains(&buffer_id));
        state.resume_autosave(buffer_id);
        assert!(!state.suspended_autosaves.contains(&buffer_id));
    }
}
