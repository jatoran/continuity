//! Codec between the high-level [`EditOp`] type and the SQLite-shaped
//! [`EditRow`].
//!
//! Encoding decomposes an op into op-kind + range + text. Decoding rebuilds
//! the op or returns [`Error::Decode`] if the row is malformed (unknown op
//! kind, missing required field, invalid selections JSON).

use continuity_buffer::{BufferId, Revision, UndoGroupId};
use continuity_text::{EditOp, Position, Range, Selection};

use crate::{store::EditRow, Error};

/// Build an [`EditRow`] from the constituent parts the core thread already
/// has on hand after applying an edit.
//
// The argument list mirrors the row's persisted columns one-for-one.
// Bundling them into a struct would just duplicate `EditRow`'s shape, so
// we accept the raw fan-in here.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn encode_edit(
    buffer_id: BufferId,
    seq: u64,
    revision: Revision,
    ts_ms: i64,
    op: &EditOp,
    removed_text: Option<&str>,
    selections_before: &[Selection],
    selections_after: &[Selection],
    undo_group_id: Option<UndoGroupId>,
    checksum_after: u64,
) -> EditRow {
    let mut row = EditRow {
        buffer_id,
        seq,
        revision,
        ts_ms,
        op_kind: String::new(),
        range_start_line: None,
        range_start_byte: None,
        range_end_line: None,
        range_end_byte: None,
        inserted_text: None,
        removed_text: removed_text.map(str::to_owned),
        selections_before_json: encode_selections(selections_before),
        selections_after_json: encode_selections(selections_after),
        undo_group_id: undo_group_id.map(|g| g.as_uuid()),
        checksum_after,
    };
    match op {
        EditOp::Insert { at, text } => {
            row.op_kind = "insert".into();
            row.range_start_line = Some(at.line);
            row.range_start_byte = Some(at.byte_in_line);
            row.inserted_text = Some(text.clone());
            // Insert removes nothing — keep the column NULL even if a caller
            // accidentally passed `Some("")`.
            if matches!(row.removed_text.as_deref(), Some("")) {
                row.removed_text = None;
            }
        }
        EditOp::Delete { range } => {
            row.op_kind = "delete".into();
            row.range_start_line = Some(range.start.line);
            row.range_start_byte = Some(range.start.byte_in_line);
            row.range_end_line = Some(range.end.line);
            row.range_end_byte = Some(range.end.byte_in_line);
        }
        EditOp::Replace { range, text } => {
            row.op_kind = "replace".into();
            row.range_start_line = Some(range.start.line);
            row.range_start_byte = Some(range.start.byte_in_line);
            row.range_end_line = Some(range.end.line);
            row.range_end_byte = Some(range.end.byte_in_line);
            row.inserted_text = Some(text.clone());
        }
    }
    row
}

/// Decode an [`EditRow`] back into an [`EditOp`].
///
/// # Errors
///
/// Returns [`Error::Decode`] if the row's `op_kind` is not recognized, or if
/// a required field for that kind is absent.
pub fn decode_op(row: &EditRow) -> Result<EditOp, Error> {
    match row.op_kind.as_str() {
        "insert" => {
            let at = required_position(row, /* end = */ false)?;
            let text = row
                .inserted_text
                .clone()
                .ok_or_else(|| Error::Decode("insert row missing inserted_text".into()))?;
            Ok(EditOp::insert(at, text))
        }
        "delete" => {
            let start = required_position(row, false)?;
            let end = required_position(row, true)?;
            Ok(EditOp::delete(Range::new(start, end)))
        }
        "replace" => {
            let start = required_position(row, false)?;
            let end = required_position(row, true)?;
            let text = row
                .inserted_text
                .clone()
                .ok_or_else(|| Error::Decode("replace row missing inserted_text".into()))?;
            Ok(EditOp::replace(Range::new(start, end), text))
        }
        other => Err(Error::Decode(format!("unknown op_kind {other:?}"))),
    }
}

/// Decode the `selections_*_json` columns into a typed selection vector.
///
/// `None` (no JSON) decodes to an empty vector — the persistence layer treats
/// "no selection state recorded" as "no selections", not as an error.
///
/// # Errors
///
/// Returns [`Error::Decode`] when the JSON is present but malformed.
pub fn decode_selections(json: Option<&str>) -> Result<Vec<Selection>, Error> {
    match json {
        None => Ok(Vec::new()),
        Some(text) => {
            serde_json::from_str(text).map_err(|e| Error::Decode(format!("selections json: {e}")))
        }
    }
}

/// JSON-encode a slice of selections. Empty input encodes to `None` so that
/// the column can stay NULL in the database.
#[must_use]
pub fn encode_selections(sels: &[Selection]) -> Option<String> {
    if sels.is_empty() {
        None
    } else {
        // Serializing a fixed-shape struct can only fail under OOM; we treat
        // that as not-our-problem and emit an empty array.
        serde_json::to_string(sels).ok()
    }
}

fn required_position(row: &EditRow, end: bool) -> Result<Position, Error> {
    let (line, byte) = if end {
        (row.range_end_line, row.range_end_byte)
    } else {
        (row.range_start_line, row.range_start_byte)
    };
    let line = line.ok_or_else(|| {
        Error::Decode(format!(
            "row missing {} line",
            if end { "range_end" } else { "range_start" }
        ))
    })?;
    let byte = byte.ok_or_else(|| {
        Error::Decode(format!(
            "row missing {} byte",
            if end { "range_end" } else { "range_start" }
        ))
    })?;
    Ok(Position::new(line, byte))
}

#[cfg(test)]
mod tests {
    use continuity_text::SelectionKind;
    use proptest::prelude::*;

    use super::*;

    fn pos_strategy() -> impl Strategy<Value = Position> {
        (0u32..16, 0u32..32).prop_map(|(l, b)| Position::new(l, b))
    }

    fn op_strategy() -> impl Strategy<Value = EditOp> {
        prop_oneof![
            (pos_strategy(), "[a-zA-Z0-9 ]{0,32}").prop_map(|(at, text)| EditOp::insert(at, text)),
            (pos_strategy(), pos_strategy()).prop_map(|(a, b)| EditOp::delete(Range::new(a, b))),
            (pos_strategy(), pos_strategy(), "[a-zA-Z0-9 ]{0,32}")
                .prop_map(|(a, b, text)| EditOp::replace(Range::new(a, b), text)),
        ]
    }

    fn sel_strategy() -> impl Strategy<Value = Selection> {
        (pos_strategy(), pos_strategy(), 0u8..3).prop_map(|(anchor, head, k)| Selection {
            anchor,
            head,
            kind: match k {
                0 => SelectionKind::Caret,
                1 => SelectionKind::LineWise,
                _ => SelectionKind::BlockWise,
            },
        })
    }

    proptest! {
        #[test]
        fn round_trip_op(op in op_strategy()) {
            let buffer_id = BufferId::new();
            let row = encode_edit(buffer_id, 1, Revision(1), 0, &op, None, &[], &[], None, 0);
            let decoded = decode_op(&row).expect("decode");
            prop_assert_eq!(decoded, op);
        }

        #[test]
        fn round_trip_selections(sels in proptest::collection::vec(sel_strategy(), 0..4)) {
            let json = encode_selections(&sels);
            let decoded = decode_selections(json.as_deref()).expect("decode");
            prop_assert_eq!(decoded, sels);
        }
    }

    #[test]
    fn empty_selections_encode_to_none() {
        assert!(encode_selections(&[]).is_none());
    }

    #[test]
    fn unknown_op_kind_errors() {
        let mut row = encode_edit(
            BufferId::new(),
            1,
            Revision(1),
            0,
            &EditOp::insert(Position::ZERO, "x"),
            None,
            &[],
            &[],
            None,
            0,
        );
        row.op_kind = "frobnicate".into();
        assert!(matches!(decode_op(&row), Err(Error::Decode(_))));
    }

    #[test]
    fn missing_inserted_text_errors() {
        let mut row = encode_edit(
            BufferId::new(),
            1,
            Revision(1),
            0,
            &EditOp::insert(Position::ZERO, "x"),
            None,
            &[],
            &[],
            None,
            0,
        );
        row.inserted_text = None;
        assert!(matches!(decode_op(&row), Err(Error::Decode(_))));
    }
}
