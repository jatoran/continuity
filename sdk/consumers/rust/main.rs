use continuity_engine::{Engine, SelectionEdit};
use continuity_text::{Position, Selection};
use serde_json::Value;

fn main() {
    let fixture_path = std::env::args().nth(1).expect("parity fixture path");
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_path).expect("read parity fixture"),
    )
    .expect("parse parity fixture");

    let multi = &fixture["multiCursor"];
    let mut engine = Engine::new();
    let id = engine.open_buffer(multi["initialText"].as_str().expect("initial text"));
    let carets = multi["selections"]
        .as_array()
        .expect("selections")
        .iter()
        .map(|selection| {
            let head = &selection["head"];
            Selection::caret_at(Position::new(
                head["line"].as_u64().expect("line") as u32,
                head["byteInLine"].as_u64().expect("byte") as u32,
            ))
        })
        .collect();
    engine.set_selections(id, carets).expect("set carets");
    let batch = engine
        .apply_selection_edit(
            id,
            &SelectionEdit::InsertText(
                multi["insertText"].as_str().expect("insert text").into(),
            ),
            1_000,
        )
        .expect("insert")
        .expect("change batch");
    assert_eq!(
        engine.text(id).as_deref(),
        multi["expectedText"].as_str()
    );
    assert_eq!(
        batch.revision_after.get(),
        multi["expectedRevision"].as_u64().expect("revision")
    );
    let actual_carets: Vec<_> = engine
        .selections(id)
        .expect("carets")
        .iter()
        .map(|selection| vec![selection.head.line, selection.head.byte_in_line])
        .collect();
    let expected_carets: Vec<_> = multi["expectedCarets"]
        .as_array()
        .expect("expected carets")
        .iter()
        .map(|caret| {
            vec![
                caret[0].as_u64().expect("line") as u32,
                caret[1].as_u64().expect("byte") as u32,
            ]
        })
        .collect();
    assert_eq!(actual_carets, expected_carets);

    let deletion = &fixture["deleteBackward"];
    let deletion_id = engine.open_buffer(
        deletion["initialText"]
            .as_str()
            .expect("deletion initial text"),
    );
    let deletion_head = &deletion["selection"]["head"];
    engine
        .set_selections(
            deletion_id,
            vec![Selection::caret_at(Position::new(
                deletion_head["line"].as_u64().expect("deletion line") as u32,
                deletion_head["byteInLine"]
                    .as_u64()
                    .expect("deletion byte") as u32,
            ))],
        )
        .expect("set deletion caret");
    engine
        .apply_selection_edit(deletion_id, &SelectionEdit::DeleteBack, 2_000)
        .expect("delete backward");
    assert_eq!(
        engine.text(deletion_id).as_deref(),
        deletion["expectedText"].as_str()
    );
    let deletion_caret = engine
        .selections(deletion_id)
        .expect("deletion caret")
        .first()
        .expect("one deletion caret")
        .head;
    assert_eq!(
        [deletion_caret.line as u64, deletion_caret.byte_in_line as u64],
        [
            deletion["expectedCaret"][0]
                .as_u64()
                .expect("expected deletion line"),
            deletion["expectedCaret"][1]
                .as_u64()
                .expect("expected deletion byte"),
        ]
    );

    let undo = &fixture["undo"];
    let typing_id = engine.open_buffer("");
    for (index, input) in undo["typing"]
        .as_array()
        .expect("typing inputs")
        .iter()
        .enumerate()
    {
        engine
            .apply_selection_edit(
                typing_id,
                &SelectionEdit::InsertText(input.as_str().expect("typing input").into()),
                3_000 + index as i64 * 100,
            )
            .expect("typing insert");
    }
    assert_eq!(engine.text(typing_id).as_deref(), undo["expectedText"].as_str());
    engine.undo(typing_id, 3_400).expect("typing undo");
    assert_eq!(
        engine.text(typing_id).as_deref(),
        undo["expectedAfterUndo"].as_str()
    );
    engine.redo(typing_id, 3_500).expect("typing redo");
    assert_eq!(
        engine.text(typing_id).as_deref(),
        undo["expectedAfterRedo"].as_str()
    );

    let branch = &fixture["undoBranch"];
    let inputs = branch["inputs"].as_array().expect("branch inputs");
    let branch_id = engine.open_buffer("");
    for (index, input) in inputs.iter().take(2).enumerate() {
        engine
            .apply_selection_edit(
                branch_id,
                &SelectionEdit::InsertText(input.as_str().expect("branch input").into()),
                2_000 + index as i64,
            )
            .expect("branch insert");
    }
    engine.undo(branch_id, 2_002).expect("branch undo");
    engine
        .apply_selection_edit(
            branch_id,
            &SelectionEdit::InsertText(inputs[2].as_str().expect("new branch").into()),
            2_003,
        )
        .expect("replacement branch");
    assert_eq!(
        engine.text(branch_id).as_deref(),
        branch["expectedReplacement"].as_str()
    );
    engine.undo(branch_id, 2_004).expect("replacement undo");
    engine
        .redo_alternate(branch_id, 2_005)
        .expect("alternate branch");
    assert_eq!(
        engine.text(branch_id).as_deref(),
        branch["expectedAlternate"].as_str()
    );

    println!("CONTINUITY_RUST_PARITY {{\"status\":\"passed\",\"version\":1}}");
}
