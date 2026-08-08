//! Raw `wasm-bindgen` editor handle.

use continuity_buffer::{BufferId, Revision};
use continuity_engine::{ChangeBatch, Engine, IndentUnit, SelectionEdit};
use continuity_host::{apply_editor_operation, editor_operation_for_command, OperationRequest};
use continuity_text::{Position, Selection, SelectionKind};
use serde::Deserialize;
use wasm_bindgen::prelude::{wasm_bindgen, JsValue};

use crate::projection_cache::ProjectionCache;
use crate::report::{change_report, snapshot_report};

/// Internal WebAssembly editor handle wrapped by the npm TypeScript facade.
///
/// **Thread ownership:** JavaScript owns this value on the constructing agent.
/// All mutation is synchronous; the handle creates no workers or callbacks.
#[wasm_bindgen(js_name = RawEditor)]
pub struct WasmEditor {
    engine: Engine,
    buffer_id: BufferId,
    projection_cache: ProjectionCache,
}

#[wasm_bindgen(js_class = RawEditor)]
impl WasmEditor {
    /// Construct an ephemeral editor with one buffer.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(initial_value: &str) -> Self {
        Self::from_snapshot(initial_value, 0)
    }

    /// Construct an editor from host-owned text and revision state.
    #[wasm_bindgen(js_name = fromSnapshot)]
    #[must_use]
    pub fn from_snapshot(initial_value: &str, initial_revision: u64) -> Self {
        let mut engine = Engine::new();
        let buffer_id = BufferId::new();
        engine.load_buffer(buffer_id, initial_value, Revision(initial_revision));
        Self {
            engine,
            buffer_id,
            projection_cache: ProjectionCache::default(),
        }
    }

    /// Replace the current selections from a JSON array of portable selections.
    pub fn set_selections_json(&mut self, selections_json: &str) -> Result<(), JsValue> {
        let inputs: Vec<SelectionInput> = serde_json::from_str(selections_json)
            .map_err(|error| js_error("invalid selections", error))?;
        let selections = inputs
            .into_iter()
            .map(SelectionInput::try_into_selection)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| JsValue::from_str(&error))?;
        self.engine
            .set_selections(self.buffer_id, selections)
            .map_err(|error| js_error("set selections failed", error))
    }

    /// Insert text at every current selection and return the change report JSON.
    pub fn insert_text(&mut self, text: &str, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .apply_selection_edit(
                self.buffer_id,
                &SelectionEdit::InsertText(text.to_string()),
                normalize_timestamp(timestamp_ms),
            )
            .map_err(|error| js_error("insert failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Delete backward at every current selection.
    pub fn delete_backward(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .apply_selection_edit(
                self.buffer_id,
                &SelectionEdit::DeleteBack,
                normalize_timestamp(timestamp_ms),
            )
            .map_err(|error| js_error("delete backward failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Delete forward at every current selection.
    pub fn delete_forward(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .apply_selection_edit(
                self.buffer_id,
                &SelectionEdit::DeleteForward,
                normalize_timestamp(timestamp_ms),
            )
            .map_err(|error| js_error("delete forward failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Indent every selected line by the portable default tab unit.
    pub fn indent(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .apply_selection_edit(
                self.buffer_id,
                &SelectionEdit::Indent {
                    unit: IndentUnit::Tab,
                },
                normalize_timestamp(timestamp_ms),
            )
            .map_err(|error| js_error("indent failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Outdent every selected line by the portable default tab unit.
    pub fn outdent(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .apply_selection_edit(
                self.buffer_id,
                &SelectionEdit::Outdent {
                    unit: IndentUnit::Tab,
                },
                normalize_timestamp(timestamp_ms),
            )
            .map_err(|error| js_error("outdent failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Execute one storage-neutral command by its stable command identifier.
    pub fn execute_command(&mut self, command: &str, timestamp_ms: f64) -> Result<String, JsValue> {
        let operation = editor_operation_for_command(command)
            .ok_or_else(|| JsValue::from_str(&format!("unsupported editor command `{command}`")))?;
        let result = apply_editor_operation(
            &mut self.engine,
            &OperationRequest {
                buffer_id: self.buffer_id,
                expected_revision: None,
                timestamp_ms: normalize_timestamp(timestamp_ms),
                operation,
            },
        )
        .map_err(|error| js_error("command failed", error))?;
        serialize_optional_change(result.change.as_ref())
    }

    /// Reconcile the complete document only when `expected_revision` is current.
    pub fn reconcile_text_if_revision(
        &mut self,
        text: &str,
        expected_revision: u64,
        timestamp_ms: f64,
    ) -> Result<String, JsValue> {
        let batch = self
            .engine
            .reconcile_text_if_revision(
                self.buffer_id,
                Revision(expected_revision),
                text,
                normalize_timestamp(timestamp_ms),
            )
            .map_err(|error| js_error("reconcile failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Undo the current branch's latest group.
    pub fn undo(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .undo(self.buffer_id, normalize_timestamp(timestamp_ms))
            .map_err(|error| js_error("undo failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Redo the most-recent child branch.
    pub fn redo(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .redo(self.buffer_id, normalize_timestamp(timestamp_ms))
            .map_err(|error| js_error("redo failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Redo the alternate child branch.
    pub fn redo_alternate(&mut self, timestamp_ms: f64) -> Result<String, JsValue> {
        let batch = self
            .engine
            .redo_alternate(self.buffer_id, normalize_timestamp(timestamp_ms))
            .map_err(|error| js_error("alternate redo failed", error))?;
        serialize_optional_change(batch.as_ref())
    }

    /// Return current text, revision, and normalized selections as JSON.
    pub fn snapshot_json(&self) -> Result<String, JsValue> {
        let report = snapshot_report(&self.engine, self.buffer_id)
            .ok_or_else(|| JsValue::from_str("buffer is no longer open"))?;
        serde_json::to_string(&report).map_err(|error| js_error("snapshot encode failed", error))
    }

    /// Compute Markdown decoration and display projection synchronously.
    pub fn projection_json(&mut self) -> Result<String, JsValue> {
        let snapshot = self
            .engine
            .snapshot(self.buffer_id)
            .ok_or_else(|| JsValue::from_str("buffer is no longer open"))?;
        self.cached_projection_json(&snapshot.rope, true)
    }

    /// Compute the compact Markdown projection used by browser presentation.
    pub fn presentation_json(&mut self) -> Result<String, JsValue> {
        let snapshot = self
            .engine
            .snapshot(self.buffer_id)
            .ok_or_else(|| JsValue::from_str("buffer is no longer open"))?;
        self.cached_projection_json(&snapshot.rope, false)
    }

    /// Compute only a source-line range for an existing browser projection.
    pub fn presentation_range_json(
        &mut self,
        start_line: u32,
        end_line: u32,
    ) -> Result<String, JsValue> {
        let snapshot = self
            .engine
            .snapshot(self.buffer_id)
            .ok_or_else(|| JsValue::from_str("buffer is no longer open"))?;
        let since_revision = self
            .projection_cache
            .revision()
            .unwrap_or(snapshot.rope.revision().get());
        let (deltas, is_covered) = self
            .engine
            .deltas_with_points_since(self.buffer_id, since_revision);
        self.projection_cache
            .range_report_json(&snapshot.rope, &deltas, is_covered, start_line, end_line)
            .map_err(|error| JsValue::from_str(&error))
    }

    /// Serialize the undo history, keeping only the newest `max_groups`
    /// groups when that is non-zero.
    pub fn history_json(&self, max_groups: u32) -> Result<String, JsValue> {
        let state = crate::history::capture_history(&self.engine, self.buffer_id, max_groups)
            .ok_or_else(|| JsValue::from_str("buffer is no longer open"))?;
        serde_json::to_string(&state).map_err(|error| js_error("history encode failed", error))
    }

    /// Adopt a serialized undo history captured from this same content.
    pub fn restore_history_json(&mut self, history_json: &str) -> Result<(), JsValue> {
        let state: crate::history::HistoryState = serde_json::from_str(history_json)
            .map_err(|error| js_error("invalid history", error))?;
        crate::history::restore_history(&mut self.engine, self.buffer_id, state)
            .map_err(|error| JsValue::from_str(&error))
    }

    /// Current WebAssembly linear-memory allocation in bytes.
    #[must_use]
    pub fn linear_memory_bytes(&self) -> u32 {
        linear_memory_bytes()
    }
}

impl WasmEditor {
    fn cached_projection_json(
        &mut self,
        snapshot: &continuity_buffer::RopeSnapshot,
        should_include_mappings: bool,
    ) -> Result<String, JsValue> {
        let since_revision = self
            .projection_cache
            .revision()
            .unwrap_or(snapshot.revision().get());
        let (deltas, is_covered) = self
            .engine
            .deltas_with_points_since(self.buffer_id, since_revision);
        self.projection_cache
            .report_json(snapshot, &deltas, is_covered, should_include_mappings)
            .map_err(|error| JsValue::from_str(&error))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionInput {
    line: u32,
    byte_in_line: u32,
}

#[derive(Deserialize)]
struct SelectionInput {
    anchor: PositionInput,
    head: PositionInput,
    kind: String,
}

impl SelectionInput {
    fn try_into_selection(self) -> Result<Selection, String> {
        let kind = match self.kind.as_str() {
            "caret" => SelectionKind::Caret,
            "lineWise" => SelectionKind::LineWise,
            "blockWise" => SelectionKind::BlockWise,
            other => return Err(format!("invalid selection kind `{other}`")),
        };
        Ok(Selection::new(
            Position::new(self.anchor.line, self.anchor.byte_in_line),
            Position::new(self.head.line, self.head.byte_in_line),
            kind,
        ))
    }
}

fn serialize_optional_change(batch: Option<&ChangeBatch>) -> Result<String, JsValue> {
    let report = batch.map(change_report);
    serde_json::to_string(&report).map_err(|error| js_error("change encode failed", error))
}

fn normalize_timestamp(timestamp_ms: f64) -> i64 {
    if timestamp_ms.is_finite() {
        timestamp_ms.clamp(i64::MIN as f64, i64::MAX as f64) as i64
    } else {
        0
    }
}

fn js_error(context: &str, error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&format!("{context}: {error}"))
}

#[cfg(target_arch = "wasm32")]
fn linear_memory_bytes() -> u32 {
    use wasm_bindgen::JsCast;

    let memory = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
    let buffer = memory.buffer().unchecked_into::<js_sys::ArrayBuffer>();
    buffer.byte_length()
}

#[cfg(not(target_arch = "wasm32"))]
fn linear_memory_bytes() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::WasmEditor;

    #[test]
    fn host_snapshot_revision_is_restored() {
        let editor = WasmEditor::from_snapshot("persisted", 41);
        let snapshot = editor.snapshot_json().unwrap();
        assert!(snapshot.contains("\"revision\":41"));
    }

    #[test]
    fn default_indent_and_outdent_use_shared_line_operations() {
        let mut editor = WasmEditor::from_snapshot("alpha", 0);
        editor.indent(1.0).unwrap();
        assert!(editor
            .snapshot_json()
            .unwrap()
            .contains("\"text\":\"\\talpha\""));
        editor.outdent(2.0).unwrap();
        let snapshot = editor.snapshot_json().unwrap();
        assert!(snapshot.contains("\"text\":\"alpha\""));
        assert!(snapshot.contains("\"revision\":2"));
    }

    #[test]
    fn command_identifier_executes_the_shared_operation() {
        let mut editor = WasmEditor::from_snapshot("task", 0);
        editor.execute_command("markdown.toggle_task", 1.0).unwrap();
        assert!(editor
            .snapshot_json()
            .unwrap()
            .contains("\"text\":\"- [ ] task\""));
    }

    fn move_caret(editor: &mut WasmEditor, byte_in_line: u32) {
        editor
            .set_selections_json(&format!(
                r#"[{{"anchor":{{"line":0,"byteInLine":{byte_in_line}}},"head":{{"line":0,"byteInLine":{byte_in_line}}},"kind":"caret"}}]"#
            ))
            .unwrap();
    }

    #[test]
    fn history_round_trip_restores_undo_across_a_fresh_editor() {
        let mut editor = WasmEditor::from_snapshot("alpha", 0);
        move_caret(&mut editor, 5);
        editor.insert_text(" beta", 1.0).unwrap();
        move_caret(&mut editor, 10);
        editor.insert_text(" gamma", 5_000.0).unwrap();
        let text = "alpha beta gamma";
        assert!(editor.snapshot_json().unwrap().contains(text));
        let history = editor.history_json(0).unwrap();

        // A remount: same content, no history.
        let mut remounted = WasmEditor::from_snapshot(text, 9);
        assert!(remounted.undo(2.0).unwrap() == "null");
        remounted.restore_history_json(&history).unwrap();
        remounted.undo(3.0).unwrap();
        assert!(remounted
            .snapshot_json()
            .unwrap()
            .contains("\"text\":\"alpha beta\""));
        remounted.redo(4.0).unwrap();
        assert!(remounted.snapshot_json().unwrap().contains(text));
    }

    #[test]
    fn history_cap_keeps_the_newest_groups() {
        let mut editor = WasmEditor::from_snapshot("", 0);
        for step in 0..5 {
            editor.insert_text("x", f64::from(step) * 1_000.0).unwrap();
        }
        let capped = editor.history_json(2).unwrap();
        assert!(capped.contains("\"isTruncated\":true"));
        let full = editor.history_json(0).unwrap();
        assert!(full.contains("\"isTruncated\":false"));
        assert!(capped.len() < full.len());
    }

    #[test]
    fn selection_commands_use_shared_word_and_line_semantics() {
        let mut editor = WasmEditor::from_snapshot("alpha beta\nsecond", 0);
        editor
            .set_selections_json(
                r#"[{"anchor":{"line":0,"byteInLine":8},"head":{"line":0,"byteInLine":8},"kind":"caret"}]"#,
            )
            .unwrap();
        editor.execute_command("editor.select_word", 1.0).unwrap();
        let word = editor.snapshot_json().unwrap();
        assert!(word.contains(
            r#""anchor":{"line":0,"byteInLine":6},"head":{"line":0,"byteInLine":10},"kind":"caret""#
        ));

        editor.execute_command("editor.select_line", 2.0).unwrap();
        let line = editor.snapshot_json().unwrap();
        assert!(line.contains(
            r#""anchor":{"line":0,"byteInLine":0},"head":{"line":0,"byteInLine":10},"kind":"lineWise""#
        ));
    }
}
