# Selections + edits

Multi-cursor + block selections + the `SelectionEdit` enum (~40 variants covering every text mutation). Every edit flows through `Context::apply_selection_edit` → `EditorHandle::apply_selection_edit` → `core::selection_edit::plan` → `apply_plan`, landing as exactly one undo group per call.

## What it is
- The single planning + apply pipeline for every buffer mutation. Each named editor action becomes a `SelectionEdit` variant; the planner produces an ordered `SelectionEditPlan` of `EditOp`s in descending byte order plus an explicit `selections_after` list; the apply step lands them as one undo group.

## Key concepts
- **`SelectionEdit`** — ~60-variant enum (canonical list + planner routing: [`.docs/generated/SELECTION_EDITS.md`](../../generated/SELECTION_EDITS.md); source `crates/core/src/selection_edit.rs`). Named text operations: `InsertText`, `DeleteBack`, `DeleteForward`, `InsertNewlineSmart`, `Indent`, `Outdent`, `DuplicateLine`, `MoveLineUp/Down`, `SortLines`, `ReflowParagraph`, `ChangeCase`, `SurroundSelection`, `MarkdownToggleEmphasis`, `MarkdownSetHeading`, `MarkdownRenumberList`, `TrimTrailingWhitespaceAll`, `TrimWhitespaceAll`, `ToggleBulletWithContinuationIndent`, etc. Do not maintain the variant count or full list here — cross-reference the generated table.
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
- **Insertion**: `InsertText`, `InsertNewlineAbove/Below/Smart`, `InsertPair`, `MarkdownInsertCodeFence/Link/ImageRef`. Newline-smart is list-aware (B9, `edit_list::plan_insert_newline_smart_list_aware`): on a list-item line it continues the marker; on an empty marker-only line it removes the marker and dedents. A task line (`- [ ] `/`- [x] `) continues with a fresh **unchecked** box (`- [ ] `); an empty task stub ends the list like an empty bullet. With a single caret continuing an ordered run, it also renumbers that run in the same undo group (`renumber::try_ordered_continue_with_renumber`) so `1.`/`2.` extends to `3.`, never a duplicate `2.`.
- **Deletion**: `DeleteBack`, `DeleteForward`, `DeletePair`, `DeleteWord*`, `DeleteToLine*`, `DeleteToBracket`.
- **Line ops**: `DuplicateLine`, `DuplicateSelection`, `MoveLineUp/Down`, `JoinLines`, `JoinSelectedLines`, `SortLines`, `ReverseLines`, `UniqueLines`, `ShuffleLines(seed)`, `TrimTrailingWhitespace`, `TrimTrailingWhitespaceAll`, `TrimWhitespaceAll`.
  - `MoveLineUp`/`MoveLineDown`: when the moved block and the line it swaps with all sit inside one contiguous ordered-list run at the same indent, the run is reordered **and renumbered** (`1.`,`2.`,…) as a single replacement (`edit_lines_movement::try_move_within_ordered_run`); any non-ordered or nested line in the span falls through to the verbatim block move.
  - `TrimTrailingWhitespaceAll` strips trailing whitespace only, **preserving** indentation. `TrimWhitespaceAll` (`editor.trim_whitespace`) strips leading **and** trailing whitespace per line, whole buffer, one undo group — the per-line leading strip removes indentation **by design** (`edit_line_text/trim.rs::plan_trim_whitespace_all`).
  - `JoinLines` (Vim-`J`) folds the single line below each caret. `JoinSelectedLines` (`Ctrl+Shift+J`) joins one structural level per press: adjacent content lines join with a single space (the continuation line's leading list marker — `- ` / `* ` / `+ ` / `N. ` / `N) ` and any task checkbox — is stripped), while a blank-line separator loses exactly **one** newline so sections stay separated until pressed again. The post-edit selection covers the whole rebuilt block so the chord can be repeated to converge to one line.
- **Indent / outdent**: `Indent { unit }`, `Outdent { unit }`. Caret-only on a list line → indents the *line*, not the caret (B10). Range selections shift the selection through the per-line indent/outdent deltas (`edit_indent_shift`). `Outdent` under the `Tab` unit removes one leading tab **or** up to one indent-width of leading spaces per line, so `Shift+Tab` outdents space-indented lines even when the indent unit is tabs (`edit_indent_shift::outdent_drop_len`).
- **Case + shape**: `ChangeCase(kind)`, `TransposeChars`, `TransposeWords`, `WrapAtColumn`, `ReflowParagraph`, `SurroundSelection`.
- **Markdown**: `MarkdownToggleEmphasis` — with a bare caret sitting **inside** an existing bold/italic/strike/inline-code span, it strips the enclosing delimiter pair (`emphasis::enclosing_delimiter_runs`, bold checked before italic so a caret in `**…**` isn't mis-stripped by the single-`*` pass) instead of nesting a fresh empty pair. `MarkdownSetHeading(level)`, `MarkdownCycleHeading(delta)`, `MarkdownPromoteSection`, `MarkdownDemoteSection`, `MarkdownMoveSectionUp/Down`, `MarkdownToggleBullet/Numbered/Checkbox`, `MarkdownToggleTask`, `MarkdownCycleListMarker`, `MarkdownRenumberList`, `MarkdownWrapInBlockquote`, `MarkdownStripFormatting`, `MarkdownInsertCodeFence/Link/ImageRef`. `MarkdownStripFormatting` (`crates/core/src/edit_markdown_strip.rs`) removes markdown syntax from every covered line — heading hashes, list/checkbox/blockquote prefixes, emphasis / code / strikethrough delimiters, and link/image syntax (keeping the visible text) — conservatively (intraword `_` and lone `*` survive so `snake_case` and `2 * 3` are untouched).

### Multi-line marker toggles (skip blanks, scan-then-act)
The line-prefix toggles — `ToggleBulletAtLineStart` (`Ctrl+R`, `edit_lines/toggle_bullet.rs`), `ToggleBulletWithContinuationIndent { unit }` (`Ctrl+Shift+R`, same file), `MarkdownToggleBullet/Numbered/Checkbox/Task` (`edit_markdown.rs`) — share two rules over a multi-line selection:
- **Blank / whitespace-only lines are skipped** so toggling across paragraph gaps never mints markers on the gaps. A caret on a single empty line still toggles (start a list).
- **Scan first, then one global action**: if every covered content line already has the marker, the toggle strips them all; otherwise it adds the marker only to the lines missing it and leaves the already-marked ones untouched. So a mixed selection converges to all-on with the first press, then all-off with the second (`Ctrl+E` task toggle matches `Ctrl+R` bullet behaviour). A blank gap inside the selection never forces the toggle into add-mode.
- **Ordered → bullet → plain**: `ToggleBulletAtLineStart` add-mode treats an ordered line (`N. `/`N) `) as carrying a list prefix and **replaces** it with `- ` (existing dash bullets `- `/`* `/`+ ` are left alone); strip-mode fires only when every covered line is already a dash bullet. An ordered line therefore cycles ordered → bullet → plain across two presses (marker detection reuses `edit_markdown::split_leading_list_marker`).
- **`ToggleBulletWithContinuationIndent { unit }`** behaves like `ToggleBulletAtLineStart` for a single-line selection; for a multi-line selection the add path also prepends one `unit` indent to every covered line **after the first** (turning the selection into a bulleted list whose continuation lines nest under the first item), and the strip path removes both the bullet and that indent. `unit` is read live from the dispatch context (mirrors `editor.indent`).
- **Encoding**: `SpacesToTabs { tab_width }`, `TabsToSpaces { tab_width }`, `ConvertLineEndings(LineEnding)`.

### Cursor coalescing (B1)
`coalesce_selections` runs after every `apply_plan` and inside the `SetSelections` / `MutateSelections` dispatch arms. Identical `(anchor, head, kind)` tuples are deduped while preserving order.

### Multi-cursor mouse adds (UI-layer)
Mouse-driven multi-selection lives in `crates/ui/src/selection/region_select.rs` (selection planning) + `crates/ui/src/window_mouse.rs` (dispatch). Ctrl+double-click **adds** a word range to the existing multi-selection: `add_cursor_at_pixel` drops a fresh caret at the click target, then `select_word_on_last` grows only that newest caret into a word range, leaving prior ranges untouched (it cannot reuse `select_word`, which would word-expand every range and collapse deliberate spans). Empty-selection case is a no-op; coalescing dedups the result.

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
  - lines: `crates/core/src/edit_lines.rs` (newline/duplicate/join), `crates/core/src/edit_lines_movement.rs` (move-line + ordered renumber-on-move), `crates/core/src/edit_lines/toggle_bullet.rs` (`Ctrl+R` / `Ctrl+Shift+R` bullet toggles)
  - line text: `crates/core/src/edit_line_text.rs`, `crates/core/src/edit_line_text/trim.rs` (trailing-only vs leading-and-trailing trims)
  - words: `crates/core/src/edit_words.rs`
  - lists: `crates/core/src/edit_list.rs`, `crates/core/src/edit_list/renumber.rs` (renumber + smart-newline ordered-continue)
  - markdown blocks/inline: `crates/core/src/edit_markdown.rs`, `edit_markdown_blocks.rs`, `crates/core/src/edit_markdown/emphasis.rs` (caret-inside-span strip detection)
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
