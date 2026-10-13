//! Post-command caret follow: decide whether a dispatched command moved
//! the caret or edited text in the focused tab, and so whether the
//! viewport should follow the primary caret afterwards.
//!
//! The dispatcher used to reveal the caret after *every* successful
//! command. That silently overrode every command that restores its own
//! view — a tab switch restoring its bookmark, a pane-focus switch
//! restoring the pane's scroll — and every command that changes neither the
//! caret nor the text (save, zoom, settings toggles): whenever the user had
//! scrolled away from the caret to read, the view warped back to it. The
//! follow now fires only when the command actually changed the focused
//! tab's text or selections, or swapped the buffer shown in the same tab
//! (quick open, find-in-all-buffers jump), and never when the focused pane
//! or tab changed.
//!
//! Thread ownership: UI-thread-only; reads the focused-pane mirror and one
//! editor snapshot.

use continuity_buffer::BufferId;
use continuity_text::Selection;

use crate::pane_tree::{PaneId, TabId};
use crate::Window;

/// What the focused tab looked like around a command dispatch.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CaretFollowState {
    pane: PaneId,
    tab: Option<TabId>,
    buffer_id: BufferId,
    revision: Option<u64>,
    selections: Vec<Selection>,
}

impl Window {
    /// Capture the focused pane, tab, buffer, text revision, and
    /// selections so [`should_follow_caret_after_command`] can tell what a
    /// command changed.
    pub(crate) fn capture_caret_follow_state(&self) -> CaretFollowState {
        let snapshot = self.editor.snapshot(self.buffer_id);
        CaretFollowState {
            pane: self.tree.focused,
            tab: self.tab_session.adopted_tab,
            buffer_id: self.buffer_id,
            revision: snapshot
                .as_ref()
                .map(|snap| snap.rope_snapshot().revision().get()),
            selections: snapshot
                .map(|snap| snap.selections().to_vec())
                .unwrap_or_default(),
        }
    }
}

/// `true` when `command` may place the caret, so the dispatcher should
/// compare the focused tab around it and follow the caret if it moved.
///
/// Excluded:
/// * `view.scroll_*` — the user just moved the viewport; don't snap it back
///   to the caret.
/// * `editor.move_doc_end` / `editor.extend_doc_end` — their viewport follow
///   is owned by the deferred exact-bottom snap (`pending_doc_end_scroll`);
///   the generic reveal resolves the caret row from a possibly
///   stale/partial projection and would under-scroll first and fight the
///   snap on a soft-wrapped buffer.
/// * `editor.select_all` — selecting the whole buffer leaves the primary
///   caret at the document end; the view must stay put while everything
///   highlights.
/// * `file.*` — file commands never place the caret, yet a save may still
///   change the text (`trim_trailing_whitespace_on_save` edits lines
///   anywhere in the buffer), and that must not drag the view back to an
///   off-screen caret.
#[must_use]
pub(crate) fn is_caret_follow_command(command: &str) -> bool {
    !(command.starts_with("view.scroll_")
        || command.starts_with("file.")
        || command == "editor.move_doc_end"
        || command == "editor.extend_doc_end"
        || command == "editor.select_all")
}

/// `true` when the viewport should follow the primary caret after a
/// command that turned `before` into `after`.
///
/// * Focused pane or tab changed: never. Pane and tab switches restore
///   their own saved view, which must stick even when the caret sits off
///   screen.
/// * Same tab, different buffer: always. Quick open and the
///   find-in-all-buffers jump swap the buffer under the current tab and
///   rely on the follow to bring the new caret into view.
/// * Same tab and buffer: only when the text or the selections changed.
///   A command that touched neither has nothing to reveal.
#[must_use]
pub(crate) fn should_follow_caret_after_command(
    before: &CaretFollowState,
    after: &CaretFollowState,
) -> bool {
    if before.pane != after.pane || before.tab != after.tab {
        return false;
    }
    if before.buffer_id != after.buffer_id {
        return true;
    }
    before.revision != after.revision || before.selections != after.selections
}

#[cfg(test)]
mod tests {
    use continuity_text::Position;

    use super::*;

    fn state(pane: u64, tab: u64, revision: u64, caret_line: u32) -> CaretFollowState {
        CaretFollowState {
            pane: PaneId(pane),
            tab: Some(TabId(tab)),
            buffer_id: BufferId::nil(),
            revision: Some(revision),
            selections: vec![Selection::caret_at(Position::new(caret_line, 0))],
        }
    }

    #[test]
    fn view_scroll_file_select_all_and_doc_end_never_follow() {
        for command in [
            "view.scroll_line_down",
            "file.save",
            "file.reload_external",
            "editor.select_all",
            "editor.move_doc_end",
            "editor.extend_doc_end",
        ] {
            assert!(!is_caret_follow_command(command), "{command}");
        }
        for command in ["editor.insert_char", "editor.move_line_down", "tab.next"] {
            assert!(is_caret_follow_command(command), "{command}");
        }
    }

    #[test]
    fn edit_or_caret_motion_follows_the_caret() {
        let before = state(1, 1, 5, 10);
        assert!(should_follow_caret_after_command(
            &before,
            &state(1, 1, 6, 10)
        ));
        assert!(should_follow_caret_after_command(
            &before,
            &state(1, 1, 5, 11)
        ));
    }

    #[test]
    fn command_that_changes_nothing_keeps_the_view() {
        let before = state(1, 1, 5, 10);
        assert!(!should_follow_caret_after_command(&before, &before.clone()));
    }

    #[test]
    fn tab_or_pane_switch_keeps_the_restored_view() {
        let before = state(1, 1, 5, 10);
        assert!(!should_follow_caret_after_command(
            &before,
            &state(1, 2, 9, 300)
        ));
        assert!(!should_follow_caret_after_command(
            &before,
            &state(2, 1, 9, 300)
        ));
    }

    #[test]
    fn buffer_swapped_under_the_same_tab_follows_the_caret() {
        let before = state(1, 1, 5, 10);
        let mut after = before.clone();
        after.buffer_id = BufferId::new();
        assert!(should_follow_caret_after_command(&before, &after));
    }
}
