# Search

Find bar with X-of-N count, compact mode toggles, find-and-replace, find-in-all over open buffers, goto-line, goto-heading, fuzzy quick-open. Literal queries use a `memchr::memmem` fast path; regex queries use `grep-regex`; palette-style pickers use the custom fuzzy scorer.

## What it is
- Within-buffer find / find-next / replace, command-backed find-in-all across open buffers, plus goto-line / goto-heading palette. FTS5 indexing, find-in-files, and `Ctrl+P` quick-open are dropped (Phase G6, spec delta §L#5/§L#6/§L#17).

## Key concepts
- **Find bar** — overlay attached to the focused pane (`Overlays::Find(FindBar)`). Holds the query, replacement, mode flags (case / word / regex / preserve-case / scope), the current match list, the active match index, and the target label shown beside the counter.
- **Find target** — `(PaneId, BufferId, Revision)` stamp recorded on the current match list. Navigation / replace / matches-to-cursors refresh before consuming byte ranges when the focused pane, buffer, or revision differs.
- **Compact controls** — find-bar buttons use dense labels: `Aa` case-sensitive, `|w|` whole-word, `.*` regex, `AB` preserve-case, `All` / `Sel` scope, `Cur` matches-to-cursors. Hover rows show the command name + default hotkey.
- **Regex snippets** — hover the `.*` control to show common syntax rows (`.`, `\d+`, `\w+`, `\s+`, `^`, `$`, `.*?`, `(one|two)`). Clicking a row enables regex and inserts the syntax at the query caret.
- **Find scope** — `Buffer` searches the whole buffer; `Selection` filters matches to non-empty selection byte ranges captured from the active buffer.
- **`MatchRange`** — `{ start_byte, end_byte, line }`; produced by `find_match_ranges`.
- **`find_match_ranges_dispatch`** — chooses `PatternPath::Literal` or `PatternPath::Regex` for one query and returns `DispatchResult { matches, path, elapsed_us }`.
- **`LiteralMatcher`** — `memchr::memmem` matcher for literal-mode queries. Supports ASCII case-insensitive search, non-overlapping matches, whole-word post-filtering, and chunk-boundary-safe scanning substrate.
- **`find_match_ranges`** — `grep-regex`-based `RegexMatcher` branch; still used when the regex toggle is on or Unicode case-folding is required.
- **`fuzzy_match`** — palette / quick-open subsequence scoring (for the still-supported palette modes).

## Operations
- **Find** (`editor.find` → `Window::open_find` → `Overlays::Find`):
  - User types into the bar; `recompute_find_matches` re-runs the dispatcher on every keystroke and on every buffer revision change.
  - Re-dispatching `editor.find` while the bar is open forces find-only mode and focuses the query field, even if the per-buffer memento last used replace mode.
  - **Selection seed + select-all on open.** `Window::open_find_impl` seeds the query from the active selection when it is a single non-collapsed, single-line, non-blank, ≤256-byte range (`find_query_seed_from_selection`), and select-alls the focused field on **every** open — whether the bar was already open, reopened from the per-buffer memento, or seeded — so the next typed character overtypes the existing query. (Both `editor.find` and `editor.replace` share this; `editor.replace` also seeds.) Multi-line selections seed nothing — they become the find-in-selection scope instead.
  - Pane / tab / focused-buffer changes retarget the open bar to the new focused pane, recompute the count/highlights, and update the footer label (`P<n>: <tab label>`) without jumping the caret or scrolling.
  - `Enter` / `F3` → `step(1)` jumps to the next match; `Shift+F3` → previous.
  - Mode hotkeys while the bar is visible: `Alt+C` case, `Alt+W` word, `Alt+R` regex, `Alt+S` scope, `Alt+P` preserve-case.
  - `jump_to_current_find_match` sets the selection to the matched range, fires `maybe_trigger_jump_glow` (B6) + `maybe_start_caret_tween` (B7) for the destination.
  - `Esc` dismisses via the universal-dismiss chain.
- **Replace** (`editor.replace` → `Overlays::Find` with replace focus):
  - Re-dispatching `editor.replace` while the bar is open shows the replace field and focuses it, preserving query / replacement text.
  - `Ctrl+Enter` / `replace_one` replaces the current match + steps; `Ctrl+Shift+Enter` or `Ctrl+Alt+Enter` / `replace_all` replaces every match in one undo group.
  - Preserve-case (`AB`, `Alt+P`) adapts the replacement to the matched text shape: all-uppercase, all-lowercase, and title-case matches transform the replacement; mixed-case matches use the raw replacement.
  - δ.3 — `replace_all` raises a transient `FileBanner` on completion. Text: `"Replaced N matches (Ctrl+Z to undo)"` (singular for N=1) or `"No matches to replace"` for the zero-match case. The formatter is the free function `window_find_replace::replace_all_banner_text` so it is unit-testable without spinning up a Window.
- **Goto line** (`editor.goto_line` → `Overlays::GotoLine`):
  - User types `42`; `confirm` sets selection to `(line: 41, byte_in_line: 0)`.
- **Goto heading** (`editor.goto_heading` → `Overlays::GotoHeading`):
  - Populated from `Decorations::headings`; `fuzzy_match` over heading text; `confirm` sets selection to `(heading.line, 0)`.
- **Find in all** (`editor.find_in_all` → `Overlays::FindInAll`):
  - Searches open buffers through `recompute_find_in_all`, using the same dispatcher and aggregating total matches into one `event:find_pattern` line. The default `Ctrl+Shift+F` binding is removed; the command surface remains.
- **Multi-cursor via search** (Phase G3): with the find bar open and N matches, `Alt+Enter` converts every match into a cursor.

## Match-state persistence (Phase G2)
Per-buffer: last query, replacement, mode flags, scope. Restored when the bar opens in that buffer. While the bar stays open, pane/tab focus changes keep the active query/replacement/modes and retarget only the match set; per-buffer mementos do not hot-swap into the live search session. Setting `find.persist_per_buffer` (default `true`) disables. The bar does **not** auto-fill from the word under the caret (decisions §G2 override) — but it *does* seed from an explicit highlighted selection (§ Find), and it select-alls the focused field on every open so the restored memento query is fully highlighted and overtypable.

Selection scope is recaptured from the newly focused buffer whenever the find target changes. Old selection byte ranges never cross buffers.

## δ.3 — Regex compile feedback

`FindBar` carries a `regex_error: Option<String>` field that captures `continuity_search::Error::InvalidRegex(msg)` returned by `find_match_ranges`. The X-of-N counter renders the error message via `match_label()` instead of `"no matches"` when `regex_error.is_some()`, so the user can distinguish a broken pattern from a legitimately empty match set without an extra surface. The field self-clears on the next successful (or non-regex) `recompute_find_matches`, so the user's typing recovers the indicator automatically.

The choice to surface this *inside* the find bar (rather than as a `FileBanner`) avoids contention with banners raised by other operations (recovery, file watcher, persist failures, etc.) — the find bar already owns its own feedback slot.

## Performance notes

- Dense search-minimap ticks are bucketed to at most one mark per vertical DIP while preserving the active match tick. Normal-sized match sets still render one tick per match.
- Large replace-all plans use one full-buffer `EditOp::Replace` once the match count reaches the planning threshold, avoiding hundreds or thousands of core-thread edit applications and persistence rows. Smaller replace-all plans stay as per-match descending ops so ordinary edits keep narrow inverse ranges.

## Removed (spec delta)
- `Ctrl+P` quick-open palette — replaced by `Ctrl+O` native file dialog (Phase D9).
- default `Ctrl+Shift+F` find-in-all-buffers binding — removed (Phase G6).
- find-in-files — removed.
- FTS5 cross-buffer content search — removed (`fts_buffers` virtual table dropped from schema).

`grep-regex` + `grep-searcher` stay for the regex branch.

## API surface
- `crates/search/src/dispatcher.rs::{find_match_ranges_dispatch, classify_pattern, DispatchResult, PatternPath}`.
- `crates/search/src/literal.rs::{LiteralMatcher, LiteralMatchIter, is_ascii_word_boundary}`.
- `crates/search/src/regex.rs::{find_match_ranges, escape_literal, MatchRange, MatchError}`.
- `crates/search/src/fuzzy.rs::fuzzy_match` (palette + goto-heading scoring).
- `crates/search/src/index.rs` — was the FTS5 index; reduced to a no-op stub after G6 (legacy module retained for future title search).
- UI overlays: `crates/ui/src/find_bar.rs`, `find_regex_help.rs`, `find_replace_plan.rs`, `find_scope.rs`, `window_find_replace.rs`, `window_find_scope.rs`, `window_find_target.rs`, `overlay_render_find.rs`, `goto_overlay.rs`, `quick_open.rs` (kept for the palette mode framework; legacy quick-open binding deleted).

## Configuration
- `find.persist_per_buffer` (default `true`).
- `find.regex_default` (default `false`).
- `find.case_sensitive_default` (default `false`).
- `find.whole_word_default` (default `false`).

## Key files
- regex matcher: `crates/search/src/regex.rs`
- fuzzy scorer: `crates/search/src/fuzzy.rs`
- search-thread index (stub post-G6): `crates/search/src/index.rs`
- find bar state: `crates/ui/src/find_bar.rs`
- find bar controls / regex snippets: `crates/ui/src/find_regex_help.rs`
- replace planning: `crates/ui/src/find_replace_plan.rs`
- selection-scope filtering: `crates/ui/src/find_scope.rs`
- replace handlers: `crates/ui/src/window_find_replace.rs`
- selection-scope capture: `crates/ui/src/window_find_scope.rs`
- focused-pane retargeting: `crates/ui/src/window_find_target.rs`
- search minimap layout: `crates/ui/src/search_minimap.rs`
- Window search hooks: `crates/ui/src/window_search.rs`
- goto overlays: `crates/ui/src/goto_overlay.rs`
- overlay paint: `crates/ui/src/overlay_render.rs`, `crates/ui/src/overlay_render_find.rs`

## Relates to
- [Overlays](overlays.md) — find bar / goto-line / goto-heading all live in `Overlays`.
- [Caret presentation](caret.md) — find jumps trigger B6 glow + B7 tween.
- [Selections + edits](selection-edits.md) — replace-all lands as one undo group.
- [Decoration](decoration.md) — heading entries drive goto-heading.
