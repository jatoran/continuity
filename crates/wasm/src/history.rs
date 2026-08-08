//! Serializable undo history for hosts that unmount the editor.
//!
//! A tabbed or paned host unmounts the element when the user leaves a
//! document, which destroys the engine and with it the undo tree — so a user
//! can undo while editing a note but not after coming back to it. Scroll and
//! selection a host can already stash and restore itself; history is the one
//! piece of editor state it cannot reconstruct from outside.
//!
//! The blob is content-addressed rather than identity-addressed: it carries the
//! FNV-1a checksum of the rope it was taken from, and importing it into a
//! different document is refused. Undo replays recorded inverse ops against the
//! live rope, so a mismatched document would not fail loudly — it would rewrite
//! the wrong bytes.

use continuity_buffer::{full_walk_rope, BufferId, EditRecord, UndoGroup, UndoGroupId, UndoTree};
use continuity_engine::Engine;
use continuity_text::{EditOp, Position, Range, Selection, SelectionKind};
use serde::{Deserialize, Serialize};

/// Wire format version. Bumped when the shape changes incompatibly; an import
/// of an unknown version is refused rather than guessed at.
const HISTORY_VERSION: u32 = 1;

/// One portable source position.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionState {
    /// Zero-based source line.
    pub line: u32,
    /// Zero-based UTF-8 byte offset within the line.
    pub byte_in_line: u32,
}

/// One portable half-open source range.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RangeState {
    /// Inclusive start.
    pub start: PositionState,
    /// Exclusive end.
    pub end: PositionState,
}

/// One portable selection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectionState {
    /// Fixed end.
    pub anchor: PositionState,
    /// Moving end.
    pub head: PositionState,
    /// `caret`, `lineWise`, or `blockWise`.
    pub kind: SelectionKindState,
}

/// Portable selection flavour.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionKindState {
    /// Collapsed caret.
    Caret,
    /// Line-wise selection.
    LineWise,
    /// Block / column selection.
    BlockWise,
}

/// One portable atomic edit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditOpState {
    /// Insert `text` at `at`.
    Insert {
        /// Where the text was inserted.
        at: PositionState,
        /// What was inserted.
        text: String,
    },
    /// Delete the bytes covered by `range`.
    Delete {
        /// The removed range.
        range: RangeState,
    },
    /// Replace the bytes covered by `range` with `text`.
    Replace {
        /// The replaced range.
        range: RangeState,
        /// The replacement text.
        text: String,
    },
}

/// One recorded edit and the op that reverts it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRecordState {
    /// The op that was applied.
    pub op: EditOpState,
    /// The op that restores the pre-edit rope.
    pub inverse_op: EditOpState,
    /// Revision before the op.
    pub revision_before: u64,
    /// Revision after the op.
    pub revision_after: u64,
    /// Selections before the op.
    pub selections_before: Vec<SelectionState>,
    /// Selections after the op.
    pub selections_after: Vec<SelectionState>,
}

/// One coalesced undo group.
///
/// Parents are positions in [`HistoryState::groups`] rather than the tree's own
/// UUIDs: nothing outside the tree refers to a group id, and an index-linked
/// blob is both smaller and self-consistent after truncation drops a parent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoGroupState {
    /// Parent group index, or `null` for a root branch.
    pub parent: Option<u32>,
    /// Wall-clock millis of group creation.
    pub timestamp_ms: i64,
    /// Command that produced the group.
    pub command: String,
    /// Edits in application order.
    pub ops: Vec<EditRecordState>,
}

/// A complete portable undo history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryState {
    /// Wire format version.
    pub version: u32,
    /// Revision the history was captured at, for host bookkeeping.
    pub revision: u64,
    /// FNV-1a checksum of the rope the history belongs to, as a decimal string.
    pub checksum: String,
    /// Index of the group whose ops are currently applied, if it was retained.
    pub current_group_index: Option<u32>,
    /// Whether older groups were dropped to satisfy a group cap.
    pub is_truncated: bool,
    /// Groups in insertion order.
    pub groups: Vec<UndoGroupState>,
}

/// Capture a buffer's undo history, optionally keeping only recent groups.
///
/// `max_groups` of zero exports everything. A cap keeps the newest groups, and
/// then widens the window until at most one retained group has a dropped
/// parent: a redo from the pre-history head re-applies the most recent root
/// branch, so two competing roots could re-apply ops against a rope that never
/// held their pre-state.
pub fn capture_history(
    engine: &Engine,
    buffer_id: BufferId,
    max_groups: u32,
) -> Option<HistoryState> {
    let buffer = engine.state().get(buffer_id)?;
    let tree = buffer.undo_tree();
    let groups = tree.groups();
    let start = truncation_start(groups, max_groups as usize);
    let window = &groups[start..];
    let current_group_index = tree
        .current_id()
        .and_then(|id| window.iter().position(|group| group.id == id))
        .map(|index| index as u32);
    Some(HistoryState {
        version: HISTORY_VERSION,
        revision: buffer.revision().get(),
        checksum: full_walk_rope(buffer.rope()).to_string(),
        current_group_index,
        is_truncated: start > 0,
        groups: window
            .iter()
            .map(|group| group_state(group, window))
            .collect(),
    })
}

/// Adopt a captured history into a buffer holding the same content.
///
/// # Errors
///
/// Returns a message when the wire version is unknown or the checksum does not
/// match the buffer's current content.
pub fn restore_history(
    engine: &mut Engine,
    buffer_id: BufferId,
    state: HistoryState,
) -> Result<(), String> {
    if state.version != HISTORY_VERSION {
        return Err(format!(
            "unsupported history version {}; this build writes and reads version {HISTORY_VERSION}",
            state.version
        ));
    }
    let buffer = engine
        .state_mut()
        .get_mut(buffer_id)
        .ok_or_else(|| "buffer is no longer open".to_string())?;
    let checksum = full_walk_rope(buffer.rope()).to_string();
    if checksum != state.checksum {
        return Err(
            "history belongs to different content; restore the same text before importing it"
                .to_string(),
        );
    }
    // Fresh ids: the blob links by index, so identity is re-minted here and the
    // parent links are resolved against the ids just handed out.
    let ids: Vec<UndoGroupId> = state.groups.iter().map(|_| UndoGroupId::new()).collect();
    let current = state
        .current_group_index
        .and_then(|index| ids.get(index as usize).copied());
    let groups = state
        .groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| undo_group(group, ids[index], &ids))
        .collect();
    *buffer.undo_tree_mut() = UndoTree::restore(groups, current);
    Ok(())
}

/// First retained index for a group cap, widened until roots are unambiguous.
fn truncation_start(groups: &[UndoGroup], max_groups: usize) -> usize {
    if max_groups == 0 || groups.len() <= max_groups {
        return 0;
    }
    let mut start = groups.len() - max_groups;
    while start > 0 && dangling_root_count(&groups[start..]) > 1 {
        start -= 1;
    }
    if dangling_root_count(&groups[start..]) > 1 {
        return 0;
    }
    start
}

fn dangling_root_count(window: &[UndoGroup]) -> usize {
    window
        .iter()
        .filter(|group| {
            group
                .parent
                .is_none_or(|parent| !window.iter().any(|candidate| candidate.id == parent))
        })
        .count()
}

fn group_state(group: &UndoGroup, window: &[UndoGroup]) -> UndoGroupState {
    let parent = group.parent.and_then(|parent| {
        window
            .iter()
            .position(|candidate| candidate.id == parent)
            .map(|index| index as u32)
    });
    UndoGroupState {
        parent,
        timestamp_ms: group.timestamp_ms,
        command: group.command.clone(),
        ops: group.ops.iter().map(edit_record_state).collect(),
    }
}

fn edit_record_state(record: &EditRecord) -> EditRecordState {
    EditRecordState {
        op: edit_op_state(&record.op),
        inverse_op: edit_op_state(&record.inverse_op),
        revision_before: record.revision_before.get(),
        revision_after: record.revision_after.get(),
        selections_before: record
            .selections_before
            .iter()
            .map(selection_state)
            .collect(),
        selections_after: record
            .selections_after
            .iter()
            .map(selection_state)
            .collect(),
    }
}

fn edit_op_state(op: &EditOp) -> EditOpState {
    match op {
        EditOp::Insert { at, text } => EditOpState::Insert {
            at: position_state(*at),
            text: text.clone(),
        },
        EditOp::Delete { range } => EditOpState::Delete {
            range: range_state(*range),
        },
        EditOp::Replace { range, text } => EditOpState::Replace {
            range: range_state(*range),
            text: text.clone(),
        },
    }
}

fn position_state(position: Position) -> PositionState {
    PositionState {
        line: position.line,
        byte_in_line: position.byte_in_line,
    }
}

fn range_state(range: Range) -> RangeState {
    RangeState {
        start: position_state(range.start),
        end: position_state(range.end),
    }
}

fn selection_state(selection: &Selection) -> SelectionState {
    SelectionState {
        anchor: position_state(selection.anchor),
        head: position_state(selection.head),
        kind: match selection.kind {
            SelectionKind::Caret => SelectionKindState::Caret,
            SelectionKind::LineWise => SelectionKindState::LineWise,
            SelectionKind::BlockWise => SelectionKindState::BlockWise,
        },
    }
}

fn undo_group(state: UndoGroupState, id: UndoGroupId, ids: &[UndoGroupId]) -> UndoGroup {
    UndoGroup {
        id,
        parent: state
            .parent
            .and_then(|index| ids.get(index as usize).copied()),
        ops: state.ops.into_iter().map(edit_record).collect(),
        timestamp_ms: state.timestamp_ms,
        command: state.command,
    }
}

fn edit_record(state: EditRecordState) -> EditRecord {
    EditRecord {
        op: edit_op(state.op),
        inverse_op: edit_op(state.inverse_op),
        revision_before: continuity_buffer::Revision(state.revision_before),
        revision_after: continuity_buffer::Revision(state.revision_after),
        selections_before: state.selections_before.into_iter().map(selection).collect(),
        selections_after: state.selections_after.into_iter().map(selection).collect(),
    }
}

fn edit_op(state: EditOpState) -> EditOp {
    match state {
        EditOpState::Insert { at, text } => EditOp::Insert {
            at: position(at),
            text,
        },
        EditOpState::Delete { range } => EditOp::Delete {
            range: range_of(range),
        },
        EditOpState::Replace { range, text } => EditOp::Replace {
            range: range_of(range),
            text,
        },
    }
}

fn position(state: PositionState) -> Position {
    Position::new(state.line, state.byte_in_line)
}

fn range_of(state: RangeState) -> Range {
    Range::new(position(state.start), position(state.end))
}

fn selection(state: SelectionState) -> Selection {
    Selection::new(
        position(state.anchor),
        position(state.head),
        match state.kind {
            SelectionKindState::Caret => SelectionKind::Caret,
            SelectionKindState::LineWise => SelectionKind::LineWise,
            SelectionKindState::BlockWise => SelectionKind::BlockWise,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{capture_history, restore_history};
    use continuity_buffer::{BufferId, Revision};
    use continuity_engine::{Engine, SelectionEdit};
    use continuity_text::{Position, Selection};

    fn engine_with(text: &str) -> (Engine, BufferId) {
        let mut engine = Engine::new();
        let id = BufferId::new();
        engine.load_buffer(id, text, Revision(0));
        (engine, id)
    }

    /// Type at the document end, the way a writer produces coalescing groups.
    fn type_text(engine: &mut Engine, id: BufferId, text: &str, at_ms: i64) {
        let current = engine.text(id).expect("invariant: buffer is open");
        let line = current.matches('\n').count() as u32;
        let byte_in_line = current.rsplit('\n').next().unwrap_or("").len() as u32;
        engine
            .set_selections(
                id,
                vec![Selection::caret_at(Position::new(line, byte_in_line))],
            )
            .expect("invariant: caret is in range");
        engine
            .apply_selection_edit(id, &SelectionEdit::InsertText(text.to_string()), at_ms)
            .expect("invariant: insert applies");
    }

    #[test]
    fn import_refuses_content_the_history_was_not_taken_from() {
        let (mut source, source_id) = engine_with("alpha");
        type_text(&mut source, source_id, " beta", 1);
        let state = capture_history(&source, source_id, 0).expect("invariant: buffer is open");

        let (mut other, other_id) = engine_with("something else entirely");
        let error = restore_history(&mut other, other_id, state).expect_err("content differs");
        assert!(error.contains("different content"), "{error}");
    }

    #[test]
    fn import_refuses_an_unknown_wire_version() {
        let (source, source_id) = engine_with("alpha");
        let mut state = capture_history(&source, source_id, 0).expect("invariant: buffer is open");
        state.version = 99;
        let (mut target, target_id) = engine_with("alpha");
        let error =
            restore_history(&mut target, target_id, state).expect_err("version is unsupported");
        assert!(error.contains("unsupported history version"), "{error}");
    }

    #[test]
    fn a_capped_export_keeps_one_root_so_redo_cannot_reapply_a_stale_branch() {
        let (mut engine, id) = engine_with("");
        for step in 0..6 {
            type_text(&mut engine, id, "x", step * 1_000);
        }
        let state = capture_history(&engine, id, 2).expect("invariant: buffer is open");
        assert!(state.is_truncated);
        assert_eq!(state.groups.len(), 2);
        assert_eq!(
            state.groups.iter().filter(|g| g.parent.is_none()).count(),
            1
        );
        assert_eq!(state.current_group_index, Some(1));
    }

    #[test]
    fn a_restored_history_undoes_and_redoes_against_the_same_content() {
        let (mut source, source_id) = engine_with("alpha");
        type_text(&mut source, source_id, " beta", 1);
        type_text(&mut source, source_id, " gamma", 5_000);
        let text = source.text(source_id).expect("invariant: buffer is open");
        let state = capture_history(&source, source_id, 0).expect("invariant: buffer is open");

        let (mut target, target_id) = engine_with(&text);
        assert!(target.undo(target_id, 1).expect("undo is total").is_none());
        restore_history(&mut target, target_id, state).expect("same content");
        target.undo(target_id, 2).expect("undo applies");
        assert_eq!(target.text(target_id).as_deref(), Some("alpha beta"));
        target.redo(target_id, 3).expect("redo applies");
        assert_eq!(target.text(target_id).as_deref(), Some(text.as_str()));
    }
}
