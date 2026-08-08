# Selections + edits

Multi-cursor + block selections + the `SelectionEdit` enum. Native edits flow
through `Context::apply_selection_edit` → `EditorHandle` →
`Engine::apply_selection_edit`; direct hosts call the synchronous engine. Each
call lands as exactly one undo group.

## What it is
- The single planning + apply pipeline for every buffer mutation. Each named editor action becomes a `SelectionEdit` variant; the planner produces an ordered `SelectionEditPlan` of `EditOp`s in descending byte order plus an explicit `selections_after` list; the apply step lands them as one undo group.

## Key concepts
- **`SelectionEdit`** — canonical list + planner routing:
  [`.docs/generated/SELECTION_EDITS.md`](../../generated/SELECTION_EDITS.md);
  source `crates/engine/src/selection_edit.rs`.
- **`SelectionEditPlan`** — `{ ops: Vec<EditOp> (descending), selections_before, selections_after }`.
- **Planner** — `crate::selection_edit::plan(buf, &edit) -> Result<Option<SelectionEditPlan>, Error>`. `None` ⇒ no effect, no undo group.
- **Apply** — engine undo management mints/coalesces the group, applies each op via `Buffer::apply`, and finishes with the planned selections.
- **Coalescing** — `continuity_engine::selection_coalesce` dedups identical selections.

## Data flow

```
Command handler  → ctx.apply_selection_edit(SelectionEdit::X)
                 → Window::dispatch_selection_edit
                 → EditorMessage::ApplySelectionEdit
                 ↓
Engine::apply_selection_edit
   ├─ selection_edit::plan(buf, &edit)
   │     ├─ insert/delete/move/etc. → per-family planner module
   │     │   (edit_inline, edit_lines, edit_line_text, edit_words,
   │     │    edit_markdown, edit_markdown_blocks, edit_list,
   │     │    edit_pairs, edit_indent_shift, edit_planning helpers)
   │     └─ returns Option<SelectionEditPlan>
   └─ engine undo manager
         ├─ mint_or_coalesce_group(command_name, kind)
         ├─ for op in plan.ops: buf.apply(op) (auto-transform selections)
         ├─ undo_tree.append_record(group_id, …)
         └─ buf.set_selections(plan.selections_after)   ← overrides auto-transform
```

## Operations
- **Insertion**: `InsertText`, `InsertNewlineAbove/Below/Smart`, `InsertPair`, `MarkdownInsertCodeFence/Link/ImageRef`. Plain text insertion and multi-cursor smart newline use `edit_planning::finalize_specs_with_transformed_selections`, which walks the descending spec sequence with the same cumulative position transform as `Buffer::apply`; later carets therefore include every earlier cursor's byte/line delta. Newline-smart is list-aware (B9, `edit_list::plan_insert_newline_smart_list_aware`): on a list-item line it continues the marker; on an empty marker-only line it removes the marker and dedents. A task line (`- [ ] `/`- [x] `) continues with a fresh **unchecked** box (`- [ ] `); an empty task stub ends the list like an empty bullet. With a single caret continuing an ordered run, it also renumbers that run in the same undo group (`renumber::try_ordered_continue_with_renumber`) so `1.`/`2.` extends to `3.`, never a duplicate `2.`.
- **Deletion**: `DeleteBack`, `DeleteForward`, `DeletePair`, `DeleteWord*`, `DeleteToLine*`, `DeleteToBracket`.
- **Line ops**: `DuplicateLine`, `DuplicateSelection`, `MoveLineUp/Down`, `JoinLines`, `JoinSelectedLines`, `SortLines`, `ReverseLines`, `UniqueLines`, `ShuffleLines(seed)`, `TrimTrailingWhitespace`, `TrimTrailingWhitespaceAll`, `TrimWhitespaceAll`.
  - `MoveLineUp`/`MoveLineDown`: when the moved block and the line it swaps with all sit inside one contiguous ordered-list run at the same indent, the run is reordered **and renumbered** (`1.`,`2.`,…) as a single replacement (`edit_lines_movement::try_move_within_ordered_run`); any non-ordered or nested line in the span falls through to the verbatim block move.
  - `TrimTrailingWhitespaceAll` strips trailing whitespace only, **preserving** indentation. `TrimWhitespaceAll` (`editor.trim_whitespace`) strips leading **and** trailing whitespace per line, whole buffer, one undo group — the per-line leading strip removes indentation **by design** (`edit_line_text/trim.rs::plan_trim_whitespace_all`).
  - `JoinLines` (Vim-`J`) folds the single line below each caret. `JoinSelectedLines` (`Ctrl+Shift+J`) joins one structural level per press: adjacent content lines join with a single space (the continuation line's leading list marker — `- ` / `* ` / `+ ` / `N. ` / `N) ` and any task checkbox — is stripped), while a blank-line separator loses exactly **one** newline so sections stay separated until pressed again. The post-edit selection covers the whole rebuilt block so the chord can be repeated to converge to one line.
- **Indent / outdent**: `Indent { unit }`, `Outdent { unit }`. `Tab` always inserts one indent unit at the start of every covered line, including collapsed-caret lines; it never inserts indentation at the caret inside content. Duplicate cursors on one line still produce one prefix because `lines_covered` deduplicates line numbers. Selections shift through the per-line indent/outdent deltas (`edit_indent_shift`). `Outdent` under the `Tab` unit removes one leading tab **or** up to one indent-width of leading spaces per line, so `Shift+Tab` outdents space-indented lines even when the indent unit is tabs (`edit_indent_shift::outdent_drop_len`).
- **Case + shape**: `ChangeCase(kind)`, `TransposeChars`, `TransposeWords`, `WrapAtColumn`, `ReflowParagraph`, `SurroundSelection`.
- **Markdown**: `MarkdownToggleEmphasis` — with a bare caret sitting **inside** an existing bold/italic/strike/inline-code span, it strips the enclosing delimiter pair (`emphasis::enclosing_delimiter_runs`, bold checked before italic so a caret in `**…**` isn't mis-stripped by the single-`*` pass) instead of nesting a fresh empty pair. `MarkdownSetHeading(level)`, `MarkdownCycleHeading(delta)`, `MarkdownPromoteSection`, `MarkdownDemoteSection`, `MarkdownMoveSectionUp/Down`, `MarkdownToggleBullet/Numbered/Checkbox`, `MarkdownToggleTask`, `MarkdownCycleListMarker`, `MarkdownRenumberList`, `MarkdownWrapInBlockquote`, `MarkdownStripFormatting`, `MarkdownInsertCodeFence/Link/ImageRef`. `MarkdownStripFormatting` (`crates/engine/src/edit_markdown_strip.rs`) removes markdown syntax conservatively while preserving ordinary intraword punctuation.

### Multi-line marker toggles (skip blanks, scan-then-act)
The line-prefix toggles — `ToggleBulletAtLineStart` (`Ctrl+R`, `edit_lines/toggle_bullet.rs`), `ToggleBulletWithContinuationIndent { unit }` (`Ctrl+Shift+R`, same file), `MarkdownToggleBullet/Numbered/Checkbox/Task` (`edit_markdown.rs`) — share two rules over a multi-line selection:
- **Blank / whitespace-only lines are skipped** so toggling across paragraph gaps never mints markers on the gaps. A caret on a single empty line still toggles (start a list).
- **Scan first, then one global action**: if every covered content line already has the marker, the toggle strips them all; otherwise it adds the marker only to the lines missing it and leaves the already-marked ones untouched. So a mixed selection converges to all-on with the first press, then all-off with the second (`Ctrl+E` task toggle matches `Ctrl+R` bullet behaviour). A blank gap inside the selection never forces the toggle into add-mode.
- **Task selections follow content, not syntax columns**: adding or removing `- [ ] ` transforms every affected selection endpoint relative to the line's content start. A caret inside text stays on the same content character; a caret on a fresh empty line lands after the complete `- [ ] ` prefix, ready for typing. This planner is shared by native and embedded hosts.
- **Ordered → bullet → plain**: `ToggleBulletAtLineStart` add-mode treats an ordered line (`N. `/`N) `) as carrying a list prefix and **replaces** it with `- ` (existing dash bullets `- `/`* `/`+ ` are left alone); strip-mode fires only when every covered line is already a dash bullet. An ordered line therefore cycles ordered → bullet → plain across two presses (marker detection reuses `edit_markdown::split_leading_list_marker`).
- **`ToggleBulletWithContinuationIndent { unit }`** behaves like `ToggleBulletAtLineStart` for a single-line selection; for a multi-line selection the add path also prepends one `unit` indent to every covered line **after the first** (turning the selection into a bulleted list whose continuation lines nest under the first item), and the strip path removes both the bullet and that indent. `unit` is read live from the dispatch context (mirrors `editor.indent`).
- **Encoding**: `SpacesToTabs { tab_width }`, `TabsToSpaces { tab_width }`, `ConvertLineEndings(LineEnding)`.

### Cursor coalescing (B1)
`coalesce_selections` runs after every `apply_plan` and inside the `SetSelections` / `MutateSelections` dispatch arms. Identical `(anchor, head, kind)` tuples are deduped while preserving order.

### Multi-cursor mouse adds (UI-layer)
Mouse-driven multi-selection lives in `crates/ui/src/selection/region_select.rs` (selection planning) + `crates/ui/src/window_mouse.rs` (dispatch). Ctrl+double-click **adds** a word range to the existing multi-selection: `add_cursor_at_pixel` drops a fresh caret at the click target, then `select_word_on_last` grows only that newest caret into a word range, leaving prior ranges untouched (it cannot reuse `select_word`, which would word-expand every range and collapse deliberate spans). Double-click drag retains the original complete word while the pointer remains inside it and extends only across complete target-word boundaries; incidental pointer motion after a rapid double-click therefore cannot collapse the range to half a word. Empty-selection case is a no-op; coalescing dedups the result.

### Multi-cursor keyboard adds (UI-layer)
`Ctrl+Alt+Up/Down` adds from the uppermost/lower-most existing cursor rather than recomputing from the primary on every press, so a held or repeated chord continues growing the cursor set. The primary cursor supplies the sticky intended column across short rows. When a compatible painted `FrameDisplay` is cached, each press advances one visual row and uses the primary display-byte column, keeping cursors aligned across soft-wrapped paragraphs; the responsive cache-miss fallback advances one source line.

### Vertical motion sticky column (B2)
`ui::EditorSurface::selection` carries `intended_columns: Vec<u32>`,
`intended_display_columns: Vec<u32>`, and `intended_columns_for: Vec<Position>`
(a head fingerprint). `move_line_selection` reuses captured intent when live
heads still match; any horizontal motion/edit/click perturbs the fingerprint
and the next vertical step reseeds from live columns. The same surface state
owns bounded per-buffer last-edit rings; engine snapshots remain live-selection
truth. `editor_surface::selection_dispatch` brackets native core dispatch with
surface-local effect capture: after success it records the pre-edit caret and
returns an optional structural-edit pulse request. Desktop-only autosave,
projection scheduling, and persistence-chip timing stay in `Window`.
The pure helper `selection_vertical::move_line_with_column` is
unit-testable headless.

## API surface
- `crates/engine/src/selection_edit.rs` — public planner, enum, plan, and supporting edit types.
- `crates/core/src/handle.rs::EditorHandle::apply_selection_edit` — UI-facing call site.
- `crates/command/src/context.rs::Context::apply_selection_edit` — default returns `Err(UnsupportedContext("apply_selection_edit"))`. `Window` impl in `crates/ui/src/window_commanding.rs` calls `note_input_now` first (B5) then forwards.

## Configuration
- `editor.caret_*` for caret presentation (B4) — independent.
- `editor.auto_pair_*` set to `false` across the board by default (Phase B8 / J7).
- `editor.trim_trailing_whitespace_on_save` (B14) — triggers `TrimTrailingWhitespaceAll` before save snapshot.

## Key files
- planner dispatch: `crates/engine/src/selection_edit.rs`
- coalescing: `crates/engine/src/selection_coalesce.rs`
- per-family planners:
  - inline: `crates/engine/src/edit_inline.rs`
  - lines: `crates/engine/src/edit_lines.rs` and `edit_lines_movement.rs`
  - line text: `crates/engine/src/edit_line_text.rs` and `edit_line_text/trim.rs`
  - words: `crates/engine/src/edit_words.rs`
  - lists: `crates/engine/src/edit_list.rs` and `edit_list/renumber.rs`
  - markdown: `crates/engine/src/edit_markdown.rs`, `edit_markdown_blocks.rs`, and responsibility submodules
  - pairs: `crates/engine/src/edit_pairs.rs`
  - indent-shift helpers: `crates/engine/src/edit_indent_shift.rs`
- planning primitives: `crates/engine/src/edit_planning.rs`
- undo/coalescing execution: `crates/engine/src/undo.rs`
- Window selection helpers: `crates/ui/src/selection.rs`, `crates/ui/src/selection_dispatch.rs`, `crates/ui/src/selection_vertical.rs`

## Relates to
- [Buffer](buffer.md) — `Buffer::apply` is the atomic mutation primitive every plan reduces to.
- [Persistence](persistence.md) — every applied op produces an `EditRecord` row.
- [Command system](command-system.md) — `SelectionEdit` variants are bound to commands; commands route to `Context::apply_selection_edit`.
- [Caret presentation](caret.md) — sticky column, blink, jump glow, motion tween all hook on edit + motion events.
