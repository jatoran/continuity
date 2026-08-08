//! Native execution of the serialized WASM compatibility fixture.

use std::sync::Arc;

use continuity_buffer::{Revision, RopeSnapshot};
use continuity_test_fixtures::parity_corpus::WASM_PARITY_FIXTURE_JSON;
use continuity_wasm::{compute_projection_report, WasmEditor};
use ropey::Rope;

#[test]
fn serialized_fixture_matches_native_engine_and_projection() {
    let fixture: serde_json::Value =
        serde_json::from_str(WASM_PARITY_FIXTURE_JSON).expect("fixture JSON");
    assert_multi_cursor_fixture(&fixture);
    assert_delete_backward_fixture(&fixture);
    assert_no_op_fixture(&fixture);
    assert_undo_fixture(&fixture);
    assert_undo_branch_fixture(&fixture);
    assert_presentation_range_matches_full_report();

    let source = fixture["projection"]["source"]
        .as_str()
        .expect("projection source");
    let snapshot = RopeSnapshot::new(Arc::new(Rope::from_str(source)), Revision(7));
    let report = compute_projection_report(&snapshot).expect("projection report");
    let report_json = serde_json::to_value(report).expect("report JSON");
    assert_eq!(report_json, fixture["projection"]["expected"]);
}

fn assert_presentation_range_matches_full_report() {
    let source = "# first\nplain\n- **third**\nlast";
    let mut editor = WasmEditor::new(source);
    let full: serde_json::Value =
        serde_json::from_str(&editor.presentation_json().expect("full presentation"))
            .expect("full presentation JSON");
    let range: serde_json::Value = serde_json::from_str(
        &editor
            .presentation_range_json(1, 3)
            .expect("range presentation"),
    )
    .expect("range presentation JSON");
    assert_eq!(range["startLine"], 1);
    assert_eq!(range["endLine"], 3);
    assert_eq!(range["lineCount"], 4);
    let full_lines = full["lines"].as_array().expect("full lines");
    assert_eq!(range["lines"], serde_json::json!(&full_lines[1..3]));
}

fn assert_multi_cursor_fixture(fixture: &serde_json::Value) {
    let case = &fixture["multiCursor"];
    let mut editor = WasmEditor::new(case["initialText"].as_str().expect("initial text"));
    editor
        .set_selections_json(&serde_json::to_string(&case["selections"]).expect("selections JSON"))
        .expect("set selections");
    let change: serde_json::Value = serde_json::from_str(
        &editor
            .insert_text(case["insertText"].as_str().expect("insert text"), 1_000.0)
            .expect("insert"),
    )
    .expect("change JSON");
    assert_eq!(change["deltas"], case["expectedDeltas"]);
    let snapshot: serde_json::Value =
        serde_json::from_str(&editor.snapshot_json().expect("snapshot")).expect("snapshot JSON");
    assert_eq!(snapshot["text"], case["expectedText"]);
    assert_eq!(snapshot["revision"], case["expectedRevision"]);
    let carets: Vec<[u64; 2]> = snapshot["selections"]
        .as_array()
        .expect("selection array")
        .iter()
        .map(|selection| {
            [
                selection["head"]["line"].as_u64().expect("caret line"),
                selection["head"]["byteInLine"]
                    .as_u64()
                    .expect("caret byte"),
            ]
        })
        .collect();
    let expected: Vec<[u64; 2]> =
        serde_json::from_value(case["expectedCarets"].clone()).expect("expected caret pairs");
    assert_eq!(carets, expected);
}

fn assert_undo_fixture(fixture: &serde_json::Value) {
    let case = &fixture["undo"];
    let mut editor = WasmEditor::new("");
    for (index, text) in case["typing"]
        .as_array()
        .expect("typing array")
        .iter()
        .enumerate()
    {
        editor
            .insert_text(
                text.as_str().expect("typing text"),
                2_000.0 + index as f64 * 100.0,
            )
            .expect("typing insert");
    }
    assert_snapshot_text(&editor, &case["expectedText"]);
    editor.undo(2_400.0).expect("undo");
    assert_snapshot_text(&editor, &case["expectedAfterUndo"]);
    editor.redo(2_500.0).expect("redo");
    assert_snapshot_text(&editor, &case["expectedAfterRedo"]);
}

fn assert_delete_backward_fixture(fixture: &serde_json::Value) {
    let case = &fixture["deleteBackward"];
    let mut editor = WasmEditor::new(case["initialText"].as_str().expect("initial text"));
    editor
        .set_selections_json(
            &serde_json::to_string(&vec![case["selection"].clone()])
                .expect("delete selection JSON"),
        )
        .expect("set delete selection");
    editor.delete_backward(1_500.0).expect("delete backward");
    let snapshot: serde_json::Value =
        serde_json::from_str(&editor.snapshot_json().expect("snapshot")).expect("snapshot JSON");
    assert_eq!(snapshot["text"], case["expectedText"]);
    assert_eq!(
        [
            snapshot["selections"][0]["head"]["line"]
                .as_u64()
                .expect("caret line"),
            snapshot["selections"][0]["head"]["byteInLine"]
                .as_u64()
                .expect("caret byte"),
        ],
        serde_json::from_value::<[u64; 2]>(case["expectedCaret"].clone()).expect("expected caret")
    );
}

fn assert_no_op_fixture(fixture: &serde_json::Value) {
    let case = &fixture["noOp"];
    let mut editor = WasmEditor::new(case["initialText"].as_str().expect("initial text"));
    let change: serde_json::Value =
        serde_json::from_str(&editor.insert_text("", 1_600.0).expect("empty insert"))
            .expect("empty insert JSON");
    assert_eq!(change["revisionAfter"], case["expectedRevision"]);
    assert_eq!(change["deltas"], case["expectedDeltas"]);
    assert_snapshot_text(&editor, &case["initialText"]);
}

fn assert_undo_branch_fixture(fixture: &serde_json::Value) {
    let case = &fixture["undoBranch"];
    let inputs = case["inputs"].as_array().expect("undo branch inputs");
    let mut editor = WasmEditor::new("");
    editor
        .insert_text(inputs[0].as_str().expect("branch prefix"), 3_000.0)
        .expect("insert branch prefix");
    editor
        .insert_text(inputs[1].as_str().expect("abandoned branch"), 3_001.0)
        .expect("insert abandoned branch");
    editor.undo(3_002.0).expect("undo abandoned branch");
    editor
        .insert_text(inputs[2].as_str().expect("replacement branch"), 3_003.0)
        .expect("insert replacement branch");
    assert_snapshot_text(&editor, &case["expectedReplacement"]);
    editor.undo(3_004.0).expect("undo replacement branch");
    editor
        .redo_alternate(3_005.0)
        .expect("redo alternate branch");
    assert_snapshot_text(&editor, &case["expectedAlternate"]);
}

fn assert_snapshot_text(editor: &WasmEditor, expected: &serde_json::Value) {
    let snapshot: serde_json::Value =
        serde_json::from_str(&editor.snapshot_json().expect("snapshot")).expect("snapshot JSON");
    assert_eq!(&snapshot["text"], expected);
}
