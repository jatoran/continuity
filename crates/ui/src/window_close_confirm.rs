//! Close-confirmation policy for tabs and the top-level window.
//!
//! Thread ownership: UI thread of one window.

use continuity_buffer::BufferId;

use crate::pane_tree::TabId;
use crate::window_file::FileBanner;
use crate::Window;

const UNSAVED_CLOSE_CONFIRM_MS: u64 = 3_000;
const UNSAVED_CLOSE_CONFIRM_BANNER: &str =
    "Press Ctrl+W again to close. Unsaved — kept in trash (recoverable).";

/// One-shot close confirmation for a dirty buffer in a pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UnsavedCloseArm {
    pub(crate) pane_id: crate::pane_tree::PaneId,
    pub(crate) buffer_id: BufferId,
    pub(crate) armed_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseConfirmDecision {
    Close,
    Arm,
}

impl Window {
    /// Returns `true` when the close should proceed immediately.
    pub(crate) fn confirm_close_tab(&mut self, tab_id: TabId) -> bool {
        let Some(tab) = self.tree.tabs.get(&tab_id).cloned() else {
            self.clear_unsaved_close_arm();
            return true;
        };
        let pane_id = self
            .tree
            .groups
            .iter()
            .find_map(|(id, group)| group.tabs.contains(&tab_id).then_some(*id))
            .unwrap_or(self.tree.focused);
        // In an autosave vault, a file-associated buffer under the vault
        // root is continuously exported and the user never opts into manual
        // saving. Closing must guarantee the on-disk file is current and
        // must never raise the "unsaved changes" prompt. Force the export
        // now and close silently. A buffer whose autosave is suspended (an
        // unresolved external-change conflict) is excluded so the prompt
        // still warns the user.
        if self.try_export_vault_tab_on_close(tab.buffer_id) {
            self.clear_unsaved_close_arm();
            return true;
        }
        // Item 9 — the close gate arms not only for file-associated dirty
        // buffers but also for untitled (non-file-associated) buffers that
        // carry typed content. `is_tab_dirty` keeps strict file-hash
        // semantics for the gutter/title dot (it returns `false` for
        // untitled buffers because they auto-persist and are never "dirty"
        // against a file), so confirmation needs the broader predicate
        // below. An empty untitled tab still closes immediately.
        let needs_confirm = self.tab_close_needs_confirmation(&tab);
        let now_ms = self.now_ms();
        match compute_close_confirm_decision(
            &mut self.unsaved_close_arm,
            pane_id,
            tab.buffer_id,
            needs_confirm,
            now_ms,
        ) {
            CloseConfirmDecision::Close => {
                self.clear_unsaved_close_banner();
                true
            }
            CloseConfirmDecision::Arm => {
                self.file_banner = Some(FileBanner::transient_for(
                    UNSAVED_CLOSE_CONFIRM_BANNER.to_string(),
                    now_ms,
                    UNSAVED_CLOSE_CONFIRM_MS,
                ));
                false
            }
        }
    }

    /// Convenience wrapper for the keyboard / X-button / middle-click paths
    /// that always target the focused group's active tab.
    pub(crate) fn confirm_close_active_tab(&mut self) -> bool {
        let Some(active) = self.tree.groups.get(&self.tree.focused).map(|g| g.active) else {
            return true;
        };
        self.confirm_close_tab(active)
    }

    /// Always returns `true` — window close proceeds unconditionally.
    /// When any tab carries unsaved typing the buffers still hit the
    /// 30-day trash; recovery is via the Recently Closed browser.
    pub(crate) fn confirm_close_window(&self) -> bool {
        true
    }

    /// Item 9 — whether closing `tab` should arm the one-shot confirmation
    /// banner.
    ///
    /// A file-associated buffer follows the strict file-hash dirty check
    /// ([`crate::window_paint_builders::is_tab_dirty`]). A non-file-
    /// associated (ephemeral / untitled) buffer is never "dirty" against a
    /// file, but losing typed content without a prompt is still a footgun —
    /// so a non-empty untitled buffer (rope has bytes, or its revision has
    /// advanced past the initial empty state) also arms the gate. An empty
    /// untitled tab returns `false` and closes immediately.
    fn tab_close_needs_confirmation(&self, tab: &crate::pane_tree::Tab) -> bool {
        let is_dirty = crate::window_paint_builders::is_tab_dirty(self, tab);
        let Some(snap) = self.editor.snapshot(tab.buffer_id) else {
            return false;
        };
        let rope_snapshot = snap.rope_snapshot();
        let has_content = rope_snapshot.rope().len_bytes() > 0;
        let edited = rope_snapshot.revision().get() > 0;
        let is_file_associated = tab.file_associated || snap.file.is_some();
        compute_tab_close_needs_confirmation(is_dirty, is_file_associated, has_content, edited)
    }

    /// When `tab`'s buffer is a file-associated, non-suspended member of an
    /// active autosave vault, force its continuous export to disk and report
    /// that the close may proceed silently (no unsaved-changes prompt).
    /// Returns `false` for ephemeral buffers, non-vault files, ignored
    /// paths, vaults with autosave disabled, or a buffer whose autosave is
    /// suspended by an unresolved conflict — those keep the normal gate.
    fn try_export_vault_tab_on_close(&mut self, buffer_id: BufferId) -> bool {
        let is_active = self.vault.is_active();
        let autosave_on = self
            .vault
            .config()
            .is_some_and(|config| config.save.autosave);
        let is_suspended = self.vault.suspended_autosaves.contains(&buffer_id);
        let is_owned = self
            .editor
            .snapshot(buffer_id)
            .and_then(|snapshot| snapshot.file)
            .is_some_and(|file| self.vault.owns_file(&file.path));
        if !should_export_vault_buffer_on_close(is_active, autosave_on, is_owned, is_suspended) {
            return false;
        }
        // `schedule` records the latest revision; the forced flush discovers
        // and dispatches every dirty vault buffer (a no-op when already
        // exported). The write is captured now and drains through the
        // file-I/O worker even after the tab is gone.
        self.schedule_vault_autosave(buffer_id);
        self.flush_due_vault_autosaves(true);
        true
    }

    pub(crate) fn clear_unsaved_close_arm(&mut self) {
        if clear_unsaved_close_arm_slot(&mut self.unsaved_close_arm) {
            self.clear_unsaved_close_banner();
        }
    }

    fn clear_unsaved_close_banner(&mut self) {
        if self
            .file_banner
            .as_ref()
            .is_some_and(|banner| banner.has_text(UNSAVED_CLOSE_CONFIRM_BANNER))
        {
            self.file_banner = None;
        }
    }
}

/// Whether closing a tab should silently force a vault export instead of
/// running the ordinary unsaved-changes gate. True only for a file that is
/// an active, autosave-enabled, non-suspended member of the vault.
fn should_export_vault_buffer_on_close(
    is_active: bool,
    autosave_on: bool,
    is_owned: bool,
    is_suspended: bool,
) -> bool {
    is_active && autosave_on && is_owned && !is_suspended
}

fn compute_tab_close_needs_confirmation(
    is_dirty: bool,
    is_file_associated: bool,
    has_content: bool,
    was_edited: bool,
) -> bool {
    is_dirty || (!is_file_associated && (has_content || was_edited))
}

fn compute_close_confirm_decision(
    arm: &mut Option<UnsavedCloseArm>,
    pane_id: crate::pane_tree::PaneId,
    buffer_id: BufferId,
    is_dirty: bool,
    now_ms: u64,
) -> CloseConfirmDecision {
    if !is_dirty {
        *arm = None;
        return CloseConfirmDecision::Close;
    }
    if let Some(current) = arm.as_ref() {
        let same_target = current.pane_id == pane_id && current.buffer_id == buffer_id;
        let elapsed_ms = now_ms.saturating_sub(current.armed_at_ms);
        if same_target && elapsed_ms <= UNSAVED_CLOSE_CONFIRM_MS {
            *arm = None;
            return CloseConfirmDecision::Close;
        }
    }
    *arm = Some(UnsavedCloseArm {
        pane_id,
        buffer_id,
        armed_at_ms: now_ms,
    });
    CloseConfirmDecision::Arm
}

fn clear_unsaved_close_arm_slot(arm: &mut Option<UnsavedCloseArm>) -> bool {
    arm.take().is_some()
}

#[cfg(test)]
mod tests {
    use continuity_buffer::BufferId;

    use super::{
        clear_unsaved_close_arm_slot, compute_close_confirm_decision,
        compute_tab_close_needs_confirmation, should_export_vault_buffer_on_close,
        CloseConfirmDecision, UnsavedCloseArm, UNSAVED_CLOSE_CONFIRM_BANNER,
        UNSAVED_CLOSE_CONFIRM_MS,
    };
    use crate::pane_tree::PaneId;

    fn target() -> (PaneId, BufferId) {
        (PaneId(7), BufferId::new())
    }

    #[test]
    fn confirm_banner_notes_trash_recovery() {
        // Item 9 — the banner must tell the user the buffer survives in the
        // trash and is recoverable, so a second Ctrl+W is not data loss.
        assert!(UNSAVED_CLOSE_CONFIRM_BANNER.contains("trash"));
        assert!(UNSAVED_CLOSE_CONFIRM_BANNER.contains("recoverable"));
        assert!(UNSAVED_CLOSE_CONFIRM_BANNER.contains("Ctrl+W"));
    }

    #[test]
    fn autosave_vault_member_closes_without_prompt() {
        // Active vault, autosave on, buffer owned and not suspended → the
        // close path force-exports and skips the unsaved-changes gate.
        assert!(should_export_vault_buffer_on_close(true, true, true, false));
    }

    #[test]
    fn suspended_or_non_vault_buffers_keep_the_gate() {
        // Suspended by an unresolved conflict → keep the prompt.
        assert!(!should_export_vault_buffer_on_close(true, true, true, true));
        // Not owned by the vault (outside root / ignored) → keep the prompt.
        assert!(!should_export_vault_buffer_on_close(
            true, true, false, false
        ));
        // Autosave disabled in the marker → keep the prompt.
        assert!(!should_export_vault_buffer_on_close(
            true, false, true, false
        ));
        // No active vault at all → keep the prompt.
        assert!(!should_export_vault_buffer_on_close(
            false, true, true, false
        ));
    }

    #[test]
    fn unchanged_file_content_closes_without_confirmation() {
        assert!(!compute_tab_close_needs_confirmation(
            false, true, true, false
        ));
    }

    #[test]
    fn dirty_file_still_requires_confirmation() {
        assert!(compute_tab_close_needs_confirmation(true, true, true, true));
    }

    #[test]
    fn dirty_close_arms_then_commits_within_window() {
        let (pane_id, buffer_id) = target();
        let mut arm = None;
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, true, 100),
            CloseConfirmDecision::Arm
        );
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, true, 200),
            CloseConfirmDecision::Close
        );
        assert!(arm.is_none());
    }

    #[test]
    fn dirty_close_timeout_rearms_as_fresh_first_press() {
        let (pane_id, buffer_id) = target();
        let mut arm = None;
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, true, 100),
            CloseConfirmDecision::Arm
        );
        let fresh_press_ms = 100 + UNSAVED_CLOSE_CONFIRM_MS + 1;
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, true, fresh_press_ms),
            CloseConfirmDecision::Arm
        );
        assert_eq!(
            arm,
            Some(UnsavedCloseArm {
                pane_id,
                buffer_id,
                armed_at_ms: fresh_press_ms,
            })
        );
    }

    #[test]
    fn typing_cancel_requires_fresh_close_press() {
        let (pane_id, buffer_id) = target();
        let mut arm = None;
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, true, 100),
            CloseConfirmDecision::Arm
        );
        assert!(clear_unsaved_close_arm_slot(&mut arm));
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, true, 200),
            CloseConfirmDecision::Arm
        );
    }

    #[test]
    fn clean_close_commits_without_arm() {
        let (pane_id, buffer_id) = target();
        let mut arm = Some(UnsavedCloseArm {
            pane_id,
            buffer_id,
            armed_at_ms: 100,
        });
        assert_eq!(
            compute_close_confirm_decision(&mut arm, pane_id, buffer_id, false, 200),
            CloseConfirmDecision::Close
        );
        assert!(arm.is_none());
    }
}
