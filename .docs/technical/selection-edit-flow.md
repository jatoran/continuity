# Selection-edit dispatch flow

Walkthrough of a keystroke that mutates the buffer. Every text edit takes this path. There is no second mutating route through `core`.

## Step-by-step

### 1. WM_CHAR or WM_KEYDOWN reaches `wndproc`
`crates/ui/src/window.rs::wndproc` routes to either `Window::on_char(code)` or `Window::on_keydown(vk)` depending on the message.

```rs
// crates/ui/src/window_commanding.rs
pub(crate) fn on_char(&mut self, code: u32) -> bool {
    if code < 0x20 { return false; }
    if code == 0x7f { return false; }
    let Some(ch) = char::from_u32(code) else { return false; };
    self.note_input_now();                       // B5: caret stays solid
    if self.overlays.is_active() {               // overlays preempt
        return self.overlay_on_char(ch);
    }
    self.dispatch_command(EDITOR_INSERT_CHAR.as_str(),
                          &Value::String(ch.to_string()))
}
```

### 2. Keymap lookup (only for `WM_KEYDOWN`)
`Window::on_keydown` builds a `KeyChord` from VK + active modifiers, then:

```rs
match self.keymap.match_sequence(&seq, self) {
    SequenceMatch::Match(binding) => self.dispatch_command(&binding.command, &Value::Null),
    SequenceMatch::Prefix         => { self.pending_chord_sequence = seq; true },
    SequenceMatch::None           => false,    // (or retry as fresh single chord if seq.len() > 1)
}
```

### 3. Registry dispatch
`Registry::dispatch(command_id, args, ctx)` resolves a handler by id + predicate and invokes it.

```rs
// crates/command/src/editor.rs
registry.register(
    EDITOR_INDENT,
    ContextPredicate::parse("editor.focused"),
    handler(|| SelectionEdit::Indent { unit: IndentUnit::Tab }),
);
```

The handler body for `editor.indent`:

```rs
Arc::new(|_, ctx| ctx.apply_selection_edit(SelectionEdit::Indent { unit: IndentUnit::Tab }))
```

### 4. `Context::apply_selection_edit`
`Context` is a trait; the only production impl is `Window`. The Window impl (in `crates/ui/src/window_commanding.rs`) calls:

```rs
fn apply_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
    self.note_input_now();                         // B5
    self.dispatch_selection_edit(edit)
}
```

`dispatch_selection_edit` (in `crates/ui/src/selection_dispatch.rs`) captures
surface-local effects, applies the edit through the editor handle, then routes
surface and desktop side effects explicitly:

```rs
pub(crate) fn dispatch_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
    let effects = SelectionDispatchEffects::capture(&edit, pre_snapshot.as_ref());
    let result = self.editor.apply_selection_edit(self.buffer_id, edit);
    result?;
    let pulse = effects.apply_to(&mut self.surface.selection, self.buffer_id);
    self.apply_native_selection_edit_effects(native_effects);
    Ok(())
}
```

`SelectionDispatchEffects` does not mutate the engine or call host services.
It owns only the surface contract: pre-edit caret memory and structural-edit
pulse intent. `window_selection_adapter` is the single native boundary that
applies the returned effects to vault/file autosave, the projection worker,
edit-pulse presentation, and SQLite-adjacent persistence status.

### 5. Crossing into `core`
`EditorHandle::apply_selection_edit` sends `EditorMessage::ApplySelectionEdit { buffer_id, edit, reply }` over `crossbeam-channel` and blocks on `reply`.

### 6. Core thread dispatch
`crates/core/src/handle/core_loop.rs` calls the synchronous engine, then gives
the returned batch to native persistence policy.

```rs
let batch = engine.apply_selection_edit(buffer_id, &edit, clock.now_ms())?;
if let Some(batch) = batch {
    record_change_batch(engine.state_mut(), trackers, bridge, persist, &batch);
}
```

### 7. Planner — `continuity_engine::selection_edit::plan`
Each `SelectionEdit` variant routes to a per-family planner. For `Indent`:

```rs
// crates/engine/src/edit_line_text.rs
pub(crate) fn plan_indent(buffer, unit) -> Result<Option<SelectionEditPlan>, Error> {
    let prefix = indent_text(unit);
    let selections_before = buffer.selections().to_vec();
    let lines = lines_covered(buffer);
    let mut specs = Vec::new();
    for &line in &lines {
        let start = buffer.rope().line_to_byte(line);
        specs.push(EditSpec::insert(buffer.rope(), start, prefix.clone())?);
    }
    let selections_after = shift_selections_after_indent(&selections_before, &lines, prefix.len());
    Ok(finalize_specs(specs, selections_before, selections_after))
}
```

The planner returns a `SelectionEditPlan { ops, selections_before, selections_after }` with ops in **descending byte order** so each `Buffer::apply` keeps pre-edit offsets valid.

For ordinary text insertion and smart-newline endpoints, `finalize_specs_with_transformed_selections` computes `selections_after` by walking those descending specs with the same position transform `Buffer::apply` will perform. This is required for multi-cursor edits: a newline at an earlier cursor changes the final line number of every later cursor.

Most planners are stateless line/range rewrites like the above, but a few branch on document structure inside the single returned plan (still one undo group): `MoveLineUp/Down` reorders **and** renumbers a contiguous ordered run when the move stays inside it (`edit_lines_movement::try_move_within_ordered_run`, else verbatim block move); `InsertNewlineSmart` with a single caret continuing an ordered run renumbers that run (`edit_list::renumber::try_ordered_continue_with_renumber`) and continues a task line with a fresh `- [ ] `; `MarkdownToggleEmphasis` strips the enclosing delimiter pair when a bare caret is inside a span (`edit_markdown::emphasis::enclosing_delimiter_runs`) rather than inserting an empty pair. See [`selection-edits.md`](../design/features/selection-edits.md) for the per-variant behavior and [`.docs/generated/SELECTION_EDITS.md`](../generated/SELECTION_EDITS.md) for the full variant→planner table.

### 8. Apply + undo group
Engine undo management mints or coalesces one undo group:

```rs
let group_id = self.mint_or_coalesce_group(buffer, command, kind, before, timestamp, ids);
for op in ops {
    changes.push(apply_recorded_op(buffer, op, before, after, group_id)?);
}
buf.set_selections(selections_after.to_vec());      // OVERRIDES the auto-transform
```

`apply_op_into_group` does:
1. `buf.capture_removed_text(op)` — snapshot the text that's about to be removed (for the inverse op).
2. `buf.apply(op)` — mutates the rope, bumps revision, auto-transforms existing selections.
3. `compute_inverse_op(op, removed, new_rope)` — builds the inverse for redo.
4. `buf.undo_tree_mut().append_record(group_id, record)` — appends the record to the tree.
5. Append the operation and checksum to storage-neutral `ChangeBatch`.

After the synchronous call, Windows core's `PersistenceBridge` assigns
database sequence numbers, encodes rows, and sends them to persist. Direct
hosts can discard or manage the same batch themselves.

After all ops, `buf.set_selections(plan.selections_after)` overrides the per-op auto-transform with the planner's explicit selection result.

### 9. Coalesce + reply
Engine selection paths call
`continuity_engine::selection_coalesce::coalesce_selections` to dedup identical
`(anchor, head, kind)` tuples.

Then the reply channel fires:

```rs
reply.send(Ok(final_revision));
let _ = event_tx.send(EditEvent::EditApplied { id: buffer_id, revision });
```

### 10. UI tick
UI threads subscribe to `EditEvent` via `EditorHandle::events()`. On `EditApplied`, the window invalidates its layout-cache rows + posts `WM_PAINT`. The decoration pool gets a new request `(buffer_id, latest_snapshot, revision)`. See [`paint-flow.md`](paint-flow.md).

## Key invariants
- `plan.ops` is descending byte order.
- `plan.selections_after` reflects the post-edit world (planner author shifts positions through their own ops — see `edit_indent_shift.rs` for the legacy line-spanning case).
- `Buffer::set_selections` always leaves at least one caret (`Selection::caret_at(Position::ZERO)` if empty).
- `Coalesce` dedups identical selections after every apply + motion. Multi-cursor double-insert can't happen.
- `apply_planner_group` is the only path that mints an `UndoGroupId`. Bypassing it bypasses undo.

## Where each step lives

| Step | File |
|---|---|
| Wndproc dispatch | `crates/ui/src/window.rs::wndproc` |
| Char / keydown | `crates/ui/src/window_commanding.rs::{on_char, on_keydown}` |
| Keymap lookup | `crates/keymap/src/lib.rs::Keymap::match_sequence` |
| Registry dispatch | `crates/command/src/registry.rs::Registry::dispatch` |
| Context impl | `crates/ui/src/window_commanding.rs` (and family modules) |
| Editor handle | `crates/core/src/handle.rs::EditorHandle::apply_selection_edit` |
| Core dispatch | `crates/core/src/handle/core_loop.rs` |
| Planner entry | `crates/engine/src/selection_edit.rs::plan` |
| Per-family planners | `crates/engine/src/edit_*.rs` (with responsibility submodules) |
| Mouse multi-cursor adds | `crates/ui/src/selection/region_select.rs` (`select_word_on_last`) + `crates/ui/src/window_mouse.rs` |
| Undo/coalescing | `crates/engine/src/undo.rs` |
| Buffer apply + auto-transform | `crates/buffer/src/buffer.rs::Buffer::{apply, SelectionTransform}` |
| Coalesce | `crates/engine/src/selection_coalesce.rs::coalesce_selections` |
