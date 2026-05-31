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

`dispatch_selection_edit` (in `crates/ui/src/selection_dispatch.rs`) applies the edit through the editor handle, then updates UI-thread state that depends on the edit landing:

```rs
pub(crate) fn dispatch_selection_edit(&mut self, edit: SelectionEdit) -> Result<(), Error> {
    let result = self.editor.apply_selection_edit(self.buffer_id, edit);
    result?;
    // update last-edit, edit-pulse, projection-worker, and persist-chip state
    Ok(())
}
```

### 5. Crossing into `core`
`EditorHandle::apply_selection_edit` sends `EditorMessage::ApplySelectionEdit { buffer_id, edit, reply }` over `crossbeam-channel` and blocks on `reply`.

### 6. Core thread dispatch
`crates/core/src/handle.rs` routes to `crate::dispatch::apply_selection_edit`.

```rs
// crates/core/src/dispatch.rs
pub fn apply_selection_edit(
    state, trackers, undo, persist, clock, policy, buffer_id, edit,
) -> Result<Option<Revision>, Error> {
    let coalesce = coalesce_kind_for(&edit);
    let command  = command_name_for(&edit);
    let buf      = state.get_mut(buffer_id).ok_or(Error::UnknownBuffer)?;
    let Some(plan) = plan(buf, &edit)? else {
        return Ok(None);              // planner: no effect → no undo group
    };
    let final_revision = undo.apply_planner_group(
        buf, &plan.ops, &plan.selections_before, &plan.selections_after,
        command, coalesce, clock.now_ms(), persist,
    )?;
    // … snapshot policy hook …
    Ok(final_revision)
}
```

### 7. Planner — `crate::selection_edit::plan`
Each `SelectionEdit` variant routes to a per-family planner. For `Indent`:

```rs
// crates/core/src/edit_line_text.rs
pub(crate) fn plan_indent(buffer, unit) -> Result<Option<SelectionEditPlan>, Error> {
    let prefix = indent_text(unit);
    let selections_before = buffer.selections().to_vec();
    let all_caret = !selections_before.is_empty()
                 && selections_before.iter().all(|s| s.is_caret());
    if all_caret {
        // ... B10 caret-on-list-line branch + legacy insert-at-caret ...
    }
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

### 8. Apply + undo group
`crates/core/src/undo.rs::UndoOrchestrator::apply_planner_group` mints (or coalesces into) one undo group:

```rs
let group_id = self.mint_or_coalesce_group(buffer_id, buf, command, coalesce_kind, selections_before, ts_ms, persist);
for op in ops {
    let revision = self.apply_op_into_group(buf, buffer_id, op, before, after, group_id, ts_ms, persist)?;
}
buf.set_selections(selections_after.to_vec());      // OVERRIDES the auto-transform
```

`apply_op_into_group` does:
1. `buf.capture_removed_text(op)` — snapshot the text that's about to be removed (for the inverse op).
2. `buf.apply(op)` — mutates the rope, bumps revision, auto-transforms existing selections.
3. `compute_inverse_op(op, removed, new_rope)` — builds the inverse for redo.
4. `buf.undo_tree_mut().append_record(group_id, record)` — appends the record to the tree.
5. `persist.enqueue_edit_row(…)` — fires off the durability message; persist thread batches.

After all ops, `buf.set_selections(plan.selections_after)` overrides the per-op auto-transform with the planner's explicit selection result.

### 9. Coalesce + reply
`buf.set_selections(...)` runs through the dispatch arms in `core::handle::*` for `SetSelections` / `MutateSelections`, which call `crate::selection_coalesce::coalesce_selections` to dedup identical `(anchor, head, kind)` tuples (Phase B1).

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
| Core dispatch | `crates/core/src/dispatch.rs::apply_selection_edit` |
| Planner entry | `crates/core/src/selection_edit.rs::plan` |
| Per-family planners | `crates/core/src/edit_*.rs` |
| Undo orchestrator | `crates/core/src/undo.rs::UndoOrchestrator` |
| Buffer apply + auto-transform | `crates/buffer/src/buffer.rs::Buffer::{apply, SelectionTransform}` |
| Coalesce | `crates/core/src/selection_coalesce.rs::coalesce_selections` |
