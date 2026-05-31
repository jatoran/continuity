# Selections + edits

Multi-cursor + block selections + the `SelectionEdit` enum (~40 variants covering every text mutation). Every edit flows through `Context::apply_selection_edit` → `EditorHandle::apply_selection_edit` → `core::selection_edit::plan` → `apply_plan`, landing as exactly one undo group per call.

## What it is
- The single planning + apply pipeline for every buffer mutation. Each named editor action becomes a `SelectionEdit` variant; the planner produces an ordered `SelectionEditPlan` of `EditOp`s in descending byte order plus an explicit `selections_after` list; the apply step lands them as one undo group.

## Key concepts
- **`SelectionEdit`** — 39+-variant enum (see `crates/core/src/selection_edit.rs`). Named text operations: `InsertText`, `DeleteBack`, `DeleteForward`, `InsertNewlineSmart`, `Indent`, `Outdent`, `DuplicateLine`, `MoveLineUp/Down`, `SortLines`, `ReflowParagraph`, `ChangeCase`, `SurroundSelection`, `MarkdownToggleEmphasis`, `MarkdownSetHeading`, `MarkdownRenumberList`, `TrimTrailingWhitespaceAll`, etc.
- **`SelectionEditPlan`** — `{ ops: Vec<EditOp> (descending), selections_before, selections_after }`.
- **Planner** — `crate::selection_edit::plan(buf, &edit) -> Result<Option<SelectionEditPlan>, Error>`. `None` ⇒ no effect, no undo group.
- **Apply** — `apply_planner_group` (in `core::undo`) mints/coalesces the group, applies each op via `Buffer::apply`, and finishes with `buf.set_selections(plan.selections_after)`.
- **Coalescing** — `core::selection_coalesce::coalesce_selections` dedups selections with identical `(anchor, head, kind)` after every edit and motion. Prevents silent multi-cursor doubling.

## Data flow

```
Command handler  → ctx.apply_selection_edit(SelectionEdit::X)
                 → Window::dispatch_selection_edit
                 → EditorMessage::ApplySelectionEdit
                 ↓
core::dispatch::apply_selection_edit
   ├─ selection_edit::plan(buf, &edit)
   │     ├─ insert/delete/move/etc. → per-family planner module
   │     │   (edit_inline, edit_lines, edit_line_text, edit_words,
   │     │    edit_markdown, edit_markdown_blocks, edit_list,
   │     │    edit_pairs, edit_indent_shift, edit_planning helpers)
   │     └─ returns Option<SelectionEditPlan>
   └─ undo::apply_planner_group
         ├─ mint_or_coalesce_group(command_name, kind)
         ├─ for op in plan.ops: buf.apply(op) (auto-transform selections)
         ├─ undo_tree.append_record(group_id, …)
         └─ buf.set_selections(plan.selections_after)   ← overrides auto-transform
```

## Operations
- **Insertion**: `InsertText`, `InsertNewlineAbove/Below/Smart`, `InsertPair`, `MarkdownInsertCodeFence/Link/ImageRef`. Newline-smart is list-aware (B9): on a list-item line it continues the marker; on an empty marker-only line it removes the marker and dedents.
- **Deletion**: `DeleteBack`, `DeleteForward`, `DeletePair`, `DeleteWord*`, `DeleteToLine*`, `DeleteToBracket`.
- **Line ops**: `DuplicateLine`, `DuplicateSelection`, `MoveLineUp/Down`, `JoinLines`, `SortLines`, `ReverseLines`, `UniqueLines`, `ShuffleLines(seed)`, `TrimTrailingWhitespace`, `TrimTrailingWhitespaceAll`.
- **Indent / outdent**: `Indent { unit }`, `Outdent { unit }`. Caret-only on a list line → indents the *line*, not the caret (B10). Range selections shift the selection through the per-line indent/outdent deltas (`edit_indent_shift`).
- **Case + shape**: `ChangeCase(kind)`, `TransposeChars`, `TransposeWords`, `WrapAtColumn`, `ReflowParagraph`, `SurroundSelection`.
- **Markdown**: `MarkdownToggleEmphasis`, `MarkdownSetHeading(level)`, `MarkdownCycleHeading(delta)`, `MarkdownPromoteSection`, `MarkdownDemoteSection`, `MarkdownMoveSectionUp/Down`, `MarkdownToggleBullet/Numbered/Checkbox`, `MarkdownCycleListMarker`, `MarkdownRenumberList`, `MarkdownWrapInBlockquote`, `MarkdownInsertCodeFence/Link/ImageRef`.
- **Encoding**: `SpacesToTabs { tab_width }`, `TabsToSpaces { tab_width }`, `ConvertLineEndings(LineEnding)`.

### Cursor coalescing (B1)
`coalesce_selections` runs after every `apply_plan` and inside the `SetSelections` / `MutateSelections` dispatch arms. Identical `(anchor, head, kind)` tuples are deduped while preserving order.

### Vertical motion sticky column (B2)
`ui::Window` carries `intended_columns: Vec<u32>` + `intended_columns_for: Vec<Position>` (a head fingerprint). `move_line_selection` reuses the captured intended columns when the live heads still match; any horizontal motion / edit / click perturbs the fingerprint and the next vertical step reseeds from the live columns. The pure helper `selection_vertical::move_line_with_column` is unit-testable headless.

## API surface
- `crates/core/src/selection_edit.rs` — public `plan(buf, &edit)`, `apply_plan(buf, &plan)`, the `SelectionEdit` enum, and the supporting `SortKind` / `CaseKind` / `IndentUnit` / `LineEnding` / `EmphasisKind` enums.
- `crates/core/src/handle.rs::EditorHandle::apply_selection_edit` — UI-facing call site.
- `crates/command/src/context.rs::Context::apply_selection_edit` — default returns `Err(UnsupportedContext("apply_selection_edit"))`. `Window` impl in `crates/ui/src/window_commanding.rs` calls `note_input_now` first (B5) then forwards.

## Configuration
- `editor.caret_*` for caret presentation (B4) — independent.
- `editor.auto_pair_*` set to `false` across the board by default (Phase B8 / J7).
- `editor.trim_trailing_whitespace_on_save` (B14) — triggers `TrimTrailingWhitespaceAll` before save snapshot.

## Key files
- planner dispatch: `crates/core/src/selection_edit.rs`
- coalescing: `crates/core/src/selection_coalesce.rs`
- per-family planners:
  - inline: `crates/core/src/edit_inline.rs`
  - lines: `crates/core/src/edit_lines.rs`
  - line text: `crates/core/src/edit_line_text.rs`
  - words: `crates/core/src/edit_words.rs`
  - lists: `crates/core/src/edit_list.rs`
  - markdown blocks/inline: `crates/core/src/edit_markdown.rs`, `edit_markdown_blocks.rs`
  - pairs: `crates/core/src/edit_pairs.rs`
  - indent-shift helpers: `crates/core/src/edit_indent_shift.rs`
- planning primitives: `crates/core/src/edit_planning.rs` (`EditSpec`, `merge_specs`, `finalize_specs`, `line_content_end`, `advance_position`)
- undo orchestrator: `crates/core/src/undo.rs`
- Window selection helpers: `crates/ui/src/selection.rs`, `crates/ui/src/selection_dispatch.rs`, `crates/ui/src/selection_vertical.rs`

## Relates to
- [Buffer](buffer.md) — `Buffer::apply` is the atomic mutation primitive every plan reduces to.
- [Persistence](persistence.md) — every applied op produces an `EditRecord` row.
- [Command system](command-system.md) — `SelectionEdit` variants are bound to commands; commands route to `Context::apply_selection_edit`.
- [Caret presentation](caret.md) — sticky column, blink, jump glow, motion tween all hook on edit + motion events.
