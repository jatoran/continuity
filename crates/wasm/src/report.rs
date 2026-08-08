//! Stable JSON report shapes shared by the raw binding and parity tests.

use continuity_buffer::BufferId;
use continuity_engine::{ChangeBatch, Engine, MutationKind};
use continuity_text::{EditOp, Position, Selection, SelectionKind};
use serde::Serialize;

/// JSON position shape emitted by the WASM adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionReport {
    /// Zero-based source line.
    pub line: u32,
    /// Zero-based UTF-8 byte offset within the line.
    pub byte_in_line: u32,
}

/// JSON selection shape emitted by the WASM adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionReport {
    /// Fixed end of the selection.
    pub anchor: PositionReport,
    /// Moving end of the selection.
    pub head: PositionReport,
    /// `caret`, `lineWise`, or `blockWise`.
    pub kind: &'static str,
}

/// One byte delta in a change report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaReport {
    /// Pre-edit source byte.
    pub at: usize,
    /// Removed source byte count.
    pub removed_bytes: usize,
    /// Inserted source byte count.
    pub inserted_bytes: usize,
}

/// One text splice in application order for incremental host mirrors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditReport {
    /// Pre-edit source byte.
    pub at: usize,
    /// Removed source byte count.
    pub removed_bytes: usize,
    /// Replacement text, empty for deletions.
    pub inserted_text: String,
    /// First source line touched before this splice.
    pub start_line: u32,
    /// Last source line touched before this splice.
    pub end_line: u32,
}

/// Storage-neutral mutation report returned to JavaScript.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeReport {
    /// Mutation family.
    pub kind: &'static str,
    /// Command identifier.
    pub command: String,
    /// Revision before the mutation.
    pub revision_before: u64,
    /// Revision after the mutation.
    pub revision_after: u64,
    /// Selections before the complete mutation.
    pub selections_before: Vec<SelectionReport>,
    /// Selections after the complete mutation.
    pub selections_after: Vec<SelectionReport>,
    /// Byte deltas in application order.
    pub deltas: Vec<DeltaReport>,
    /// Text splices in application order for host-side incremental mirrors.
    pub edits: Vec<EditReport>,
    /// Running content checksum after the mutation.
    pub checksum_after: String,
}

/// Current public editor state returned to JavaScript.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSnapshotReport {
    /// Entire source text.
    pub text: String,
    /// Current revision.
    pub revision: u64,
    /// Current normalized selections.
    pub selections: Vec<SelectionReport>,
    /// Whether mutations are rejected.
    pub is_read_only: bool,
}

pub(crate) fn change_report(batch: &ChangeBatch) -> ChangeReport {
    ChangeReport {
        kind: mutation_kind_name(batch.kind),
        command: batch.command.clone(),
        revision_before: batch.revision_before.get(),
        revision_after: batch.revision_after.get(),
        selections_before: selection_reports(&batch.selections_before),
        selections_after: selection_reports(&batch.selections_after),
        deltas: batch
            .deltas
            .iter()
            .map(|delta| DeltaReport {
                at: delta.delta.at,
                removed_bytes: delta.delta.removed_bytes,
                inserted_bytes: delta.delta.inserted_bytes,
            })
            .collect(),
        edits: batch
            .deltas
            .iter()
            .zip(&batch.changes)
            .map(|(delta, change)| {
                let (inserted_text, start_line, end_line) = match &change.op {
                    EditOp::Insert { at, text } => (text.clone(), at.line, at.line),
                    EditOp::Delete { range } => (String::new(), range.start.line, range.end.line),
                    EditOp::Replace { range, text } => {
                        (text.clone(), range.start.line, range.end.line)
                    }
                };
                EditReport {
                    at: delta.delta.at,
                    removed_bytes: delta.delta.removed_bytes,
                    inserted_text,
                    start_line,
                    end_line,
                }
            })
            .collect(),
        checksum_after: batch.checksum_after.to_string(),
    }
}

pub(crate) fn snapshot_report(
    engine: &Engine,
    buffer_id: BufferId,
) -> Option<EditorSnapshotReport> {
    let snapshot = engine.snapshot(buffer_id)?;
    Some(EditorSnapshotReport {
        text: snapshot.rope.rope().to_string(),
        revision: snapshot.rope.revision().get(),
        selections: selection_reports(&snapshot.selections),
        is_read_only: snapshot.is_read_only,
    })
}

fn selection_reports(selections: &[Selection]) -> Vec<SelectionReport> {
    selections.iter().map(selection_report).collect()
}

fn selection_report(selection: &Selection) -> SelectionReport {
    SelectionReport {
        anchor: position_report(selection.anchor),
        head: position_report(selection.head),
        kind: match selection.kind {
            SelectionKind::Caret => "caret",
            SelectionKind::LineWise => "lineWise",
            SelectionKind::BlockWise => "blockWise",
        },
    }
}

fn position_report(position: Position) -> PositionReport {
    PositionReport {
        line: position.line,
        byte_in_line: position.byte_in_line,
    }
}

fn mutation_kind_name(kind: MutationKind) -> &'static str {
    match kind {
        MutationKind::RawEdit => "rawEdit",
        MutationKind::GroupedEdit => "groupedEdit",
        MutationKind::SelectionEdit => "selectionEdit",
        MutationKind::Undo => "undo",
        MutationKind::Redo => "redo",
    }
}
