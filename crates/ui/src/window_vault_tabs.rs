//! Persist and restore a vault's open tabs (files + focus + scroll).
//!
//! A vault records its open editor tabs in `.continuity/workspace.toml` so
//! reopening the vault — via the launcher, a desktop shortcut, or `--vault` —
//! restores the same files, the focused tab, and each tab's scroll position,
//! independent of the database-backed window session.
//!
//! Scope: the focused pane's file-associated, vault-owned tabs in positional
//! order. Untitled buffers and files outside the vault are not portable and
//! are skipped; split-pane layout is not reproduced (all tabs restore into
//! one pane).
//!
//! Thread ownership: UI thread of one window.

use std::path::PathBuf;

use continuity_buffer::BufferId;
use continuity_config::{VaultTabState, VaultWorkspaceState};

use crate::vault::PendingVaultTabRestore;
use crate::window::Window;
use crate::window_config::FileOpenDisposition;

impl Window {
    /// Capture the focused pane's vault-owned file tabs in positional order,
    /// plus the index of the focused tab within that filtered list. Flushes
    /// the focused tab's live scroll into its bookmark first so the captured
    /// offset is current.
    pub(crate) fn collect_vault_open_tabs(&mut self) -> (Vec<VaultTabState>, usize) {
        self.save_tab_view_bookmark_for_adopted();
        let Some(group) = self.tree.groups.get(&self.tree.focused) else {
            return (Vec::new(), 0);
        };
        let ordered_tabs = group.tabs.clone();
        let active = group.active;
        let mut tabs = Vec::new();
        let mut focused_index = 0;
        for tab_id in ordered_tabs {
            let Some(tab) = self.tree.tabs.get(&tab_id) else {
                continue;
            };
            if !tab.file_associated {
                continue;
            }
            let buffer_id = tab.buffer_id;
            let Some(file) = self
                .editor
                .snapshot(buffer_id)
                .and_then(|snapshot| snapshot.file)
            else {
                continue;
            };
            let Some(relative) = self.vault.relative_path_for(&file.path) else {
                continue;
            };
            let scroll_y_dip = self
                .tab_session
                .view_bookmarks
                .get(&tab_id)
                .map(|bookmark| bookmark.scroll_y_dip)
                .unwrap_or(0.0)
                .max(0.0);
            if tab_id == active {
                focused_index = tabs.len();
            }
            tabs.push(VaultTabState {
                path: relative,
                scroll_y_dip,
            });
        }
        (tabs, focused_index)
    }

    /// Begin restoring a vault's saved open tabs. Issues one open per saved
    /// file and records the per-file scroll + focus so it can be applied as
    /// each open completes. No-op when there are no saved tabs, the file-I/O
    /// worker is unavailable, or the vault root is unknown.
    pub(crate) fn begin_vault_tab_restore(&mut self, workspace: &VaultWorkspaceState) {
        if workspace.open_tabs.is_empty() {
            return;
        }
        // Do not restore when this window already shows vault files: on a
        // normal restart the database session restore already installed the
        // tabs before the folder was re-inspected, and re-opening them here
        // would duplicate every tab. Restore only fires for a fresh open
        // (shortcut / launcher / `--vault` / open-here into other content).
        if self.has_open_vault_tabs() {
            return;
        }
        let Some(file_io) = self.file_io.clone() else {
            return;
        };
        // Resolve saved relative paths to absolute paths under the root.
        let mut absolute_paths = Vec::new();
        let mut scroll_by_path = std::collections::HashMap::new();
        for tab in &workspace.open_tabs {
            let Some(absolute) = self.vault.absolute_path_for(&tab.path) else {
                continue;
            };
            scroll_by_path.insert(absolute.clone(), tab.scroll_y_dip.max(0.0));
            absolute_paths.push(absolute);
        }
        if absolute_paths.is_empty() {
            return;
        }
        let focused_index = workspace.focused_tab.min(absolute_paths.len() - 1);
        let focused_path = absolute_paths.get(focused_index).cloned();

        self.vault.pending_tab_restore = Some(PendingVaultTabRestore {
            scroll_by_path,
            focused_path,
            remaining: absolute_paths.len(),
        });

        // Replace a lone blank tab in-place with the first restored file so we
        // do not leave a stray "Untitled" tab; otherwise append everything.
        let replace_first = self.is_focused_tab_blank();
        let pane = self.tree.focused;
        let hwnd = self.hwnd.0 as usize;
        let mut paths = absolute_paths.into_iter();
        if replace_first {
            if let Some(first) = paths.next() {
                let _ = file_io.open_files_with_reply(
                    vec![first],
                    Some(pane),
                    self.file_open_tx.clone(),
                    hwnd,
                    FileOpenDisposition::Preview,
                );
            }
        }
        let rest: Vec<PathBuf> = paths.collect();
        if !rest.is_empty() {
            let _ = file_io.open_files_with_reply(
                rest,
                Some(pane),
                self.file_open_tx.clone(),
                hwnd,
                FileOpenDisposition::NewTab,
            );
        }
    }

    /// Apply the pending restore for a file that just opened as a tab: seed
    /// the tab's scroll bookmark and, when this is the tab that should be
    /// focused, activate it. Decrements the outstanding-open count and clears
    /// the pending restore once every issued open has landed.
    pub(crate) fn apply_vault_tab_restore_on_open(
        &mut self,
        path: &std::path::Path,
        buffer_id: BufferId,
    ) {
        let Some(pending) = self.vault.pending_tab_restore.as_mut() else {
            return;
        };
        let Some(scroll_y_dip) = pending.scroll_by_path.remove(path) else {
            return;
        };
        pending.remaining = pending.remaining.saturating_sub(1);
        let done = pending.remaining == 0;
        let focused_path = pending.focused_path.clone();

        // Seed the tab's bookmark now; the focused tab is only activated once
        // every tab has landed, since each newly opened `NewTab` steals active
        // focus and would otherwise leave the last-opened tab focused.
        if let Some((_, tab_id)) = self.find_tab_for_buffer_in_focused_scope(buffer_id) {
            self.tab_session.view_bookmarks.insert(
                tab_id,
                crate::window_tab_strip_scroll::TabViewBookmark {
                    scroll_y_dip: scroll_y_dip.max(0.0),
                    primary_selection: None,
                },
            );
        }
        if done {
            self.finish_vault_tab_restore(focused_path);
        }
    }

    /// Finalize a completed restore: clear the ephemeral preview marker left by
    /// the first `Preview` open, activate the tab that should be focused, and
    /// drop the pending state.
    fn finish_vault_tab_restore(&mut self, focused_path: Option<PathBuf>) {
        // The first restored file replaced a blank tab via `Preview`, which
        // marks it ephemeral; clear that so the restored tab is permanent.
        self.file_tree_preview_tabs.remove(&self.tree.focused);
        if let Some(focused_path) = focused_path {
            if let Some(tab_id) = self.find_focused_pane_tab_by_path(&focused_path) {
                let scroll_y_dip = self
                    .tab_session
                    .view_bookmarks
                    .get(&tab_id)
                    .map(|bookmark| bookmark.scroll_y_dip)
                    .unwrap_or(0.0);
                self.activate_restored_tab(tab_id, scroll_y_dip);
            }
        }
        self.vault.pending_tab_restore = None;
    }

    /// The first tab in the focused pane whose buffer is associated with
    /// `path`, if any.
    fn find_focused_pane_tab_by_path(
        &self,
        path: &std::path::Path,
    ) -> Option<crate::pane_tree::TabId> {
        let group = self.tree.groups.get(&self.tree.focused)?;
        group
            .tabs
            .iter()
            .find(|tab_id| {
                self.tree
                    .tabs
                    .get(tab_id)
                    .map(|tab| tab.buffer_id)
                    .and_then(|buffer_id| self.editor.snapshot(buffer_id))
                    .and_then(|snapshot| snapshot.file)
                    .is_some_and(|file| file.path == path)
            })
            .copied()
    }

    /// Account for a restored file that failed to open (missing/unreadable) so
    /// the pending restore still completes.
    pub(crate) fn note_vault_tab_restore_failure(&mut self, path: &std::path::Path) {
        let Some(pending) = self.vault.pending_tab_restore.as_mut() else {
            return;
        };
        if pending.scroll_by_path.remove(path).is_none() {
            return;
        }
        pending.remaining = pending.remaining.saturating_sub(1);
        let focused_path = pending.focused_path.clone();
        if pending.remaining == 0 {
            self.finish_vault_tab_restore(focused_path);
        }
    }

    /// Activate `tab_id` in the focused pane and restore its bookmarked scroll
    /// without re-anchoring the caret, so the saved offset sticks.
    fn activate_restored_tab(&mut self, tab_id: crate::pane_tree::TabId, scroll_y_dip: f32) {
        if let Some(group) = self.tree.groups.get_mut(&self.tree.focused) {
            group.activate(tab_id);
        }
        self.adopt_focused_tab();
        self.surface.view.scroll_y_dip = scroll_y_dip.max(0.0);
        self.refresh_language();
        self.maybe_submit_decoration();
    }

    /// First `(pane, tab)` in the focused pane whose tab shows `buffer_id`.
    fn find_tab_for_buffer_in_focused_scope(
        &self,
        buffer_id: BufferId,
    ) -> Option<(crate::pane_tree::PaneId, crate::pane_tree::TabId)> {
        let pane = self.tree.focused;
        let group = self.tree.groups.get(&pane)?;
        group
            .tabs
            .iter()
            .find(|tab_id| {
                self.tree
                    .tabs
                    .get(tab_id)
                    .is_some_and(|tab| tab.buffer_id == buffer_id)
            })
            .map(|tab_id| (pane, *tab_id))
    }

    /// Whether any tab in this window already shows a file owned by the active
    /// vault. Used to distinguish a fresh vault open (restore wanted) from a
    /// database session restore that already installed the tabs.
    fn has_open_vault_tabs(&self) -> bool {
        self.tree.tabs.values().any(|tab| {
            tab.file_associated
                && self
                    .editor
                    .snapshot(tab.buffer_id)
                    .and_then(|snapshot| snapshot.file)
                    .is_some_and(|file| self.vault.owns_file(&file.path))
        })
    }

    /// Whether the focused pane holds a single, empty, untitled tab — the
    /// default state a fresh window / shortcut launch opens a vault into.
    fn is_focused_tab_blank(&self) -> bool {
        let Some(group) = self.tree.groups.get(&self.tree.focused) else {
            return false;
        };
        if group.tabs.len() != 1 {
            return false;
        }
        let Some(tab) = self.tree.tabs.get(&group.active) else {
            return false;
        };
        if tab.file_associated {
            return false;
        }
        match self.editor.snapshot(tab.buffer_id) {
            Some(snapshot) => {
                let rope = snapshot.rope_snapshot();
                rope.rope().len_bytes() == 0 && rope.revision().get() == 0
            }
            None => false,
        }
    }
}
