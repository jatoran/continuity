# Clipboard

Cut, copy, and paste via `CF_UNICODETEXT`. Smart-paste rewrites a clipboard URL into a markdown link / image / autolink based on context. An in-memory paste-history ring lets the user reach back N entries; nothing is persisted to disk.

## What it is
- Copy / cut / paste over `CF_UNICODETEXT`. RTF copy (`Ctrl+Shift+Alt+C`) for Word / Outlook compatibility. Smart paste (Phase B13) transforms a clipboard URL into a markdown link / image / autolink. In-memory paste history ring (no persistence per spec §12).

## Key concepts
- **`clipboard::read_text(hwnd)`** — opens the clipboard, reads `CF_UNICODETEXT`, normalizes line endings.
- **`clipboard::write_text(hwnd, &str)`** — opens the clipboard, writes `CF_UNICODETEXT`. Always normalize `\r\n` → `\n` on read; writers emit `\r\n` since Windows consumers expect it.
- **`PasteHistory`** — bounded ring (16 entries) of recently-copied text. Newest first; dedup on push. Not persisted.
- **RTF copy** — `crates/ui/src/window_link_clipboard.rs::put_clipboard_html` (variant). Builds a minimal RTF stream from the selection's `Decorations` styles.
- **Smart paste** — `crates/ui/src/smart_paste.rs::smart_paste_transform(clipboard_text, has_selection) -> Option<SmartPasteOp>`:
  - selection non-empty + URL → `SurroundSelection { open: "[", close: "](url)" }`
  - no selection + image extension URL → `InsertText("![](url)")`
  - no selection + plain URL → `InsertText("<url>")`
  - otherwise → `None` (fall through to plain paste)

## Operations

### Copy / cut
- `editor.copy` — copies selection text; multi-cursor: each selection range becomes a line in the clipboard, joined by `\n`.
- `editor.cut` — copies, then applies `SelectionEdit::DeleteBack` (or range-replace for non-collapsed selections).
- `editor.copy_as_rtf` (`Ctrl+Shift+Alt+C`) — copies as RTF text with `markdown.*` styles baked in.

### Paste (`Ctrl+V`, `editor.paste`)
1. `clipboard::read_text` returns `Option<String>`.
2. Normalize line endings → `\n`-only.
3. `smart_paste_transform` decides if this is a URL/image transform or a plain paste.
4. Dispatch the resulting `SelectionEdit`.

### Plain paste (`Ctrl+Shift+V`, `editor.paste_as_plain_text`)
- Same as `paste` minus the smart-paste step. Equivalent today because the reader only consumes `CF_UNICODETEXT` (no RTF / HTML to strip), but the binding stays as the explicit "no transform" path.

### Paste history (`Ctrl+Alt+V`, `editor.paste_from_history`)
- Optional `index` arg (default 0 = newest). Pastes the entry at that ring slot.
- Used by the (future) paste-history palette mode (decisions §J, not user-visible yet).

## RTF copy specifics
- Encodes selection text + style runs as a minimal RTF stream.
- Heading levels, bold / italic / strikethrough / code / link colors come from the active theme's `markdown.*` keys.
- Multi-cursor: each non-contiguous range becomes a separate RTF group joined by `\par`.

## Line-ending normalization
- Reader: `\r\n` → `\n`, lone `\r` → `\n`. Done in `normalize_line_endings` (window_clipboard.rs).
- Writer: when pasting back to Windows-native apps, callers re-encode as `\r\n` only on the way *out* via RTF or HTML formats. `CF_UNICODETEXT` always writes `\n`-normalized.

## API surface
- Reader / writer: `crates/win/src/clipboard.rs::{read_text, write_text, Error}`.
- Paste history: `crates/ui/src/window_clipboard.rs::PasteHistory`.
- Smart paste: `crates/ui/src/smart_paste.rs::{smart_paste_transform, SmartPasteOp}`.
- Window handlers: `crates/ui/src/window_clipboard.rs::{paste_clipboard_impl, paste_as_plain_text_impl, paste_from_history_impl}` + `crates/ui/src/window_link_clipboard.rs::{copy_impl, cut_impl, copy_as_rtf_impl}`.

## Configuration
- `clipboard.history_size` (default `16`).
- `clipboard.smart_paste_url` (default `true`).
- `clipboard.bare_url_as_autolink` (default `true` — false uses plain `<url>` form; only matters when no selection + non-image URL).

## Key files
- Win32 clipboard wrapper: `crates/win/src/clipboard.rs`
- paste history ring: `crates/ui/src/window_clipboard.rs`
- smart paste: `crates/ui/src/smart_paste.rs`
- copy / cut / RTF copy: `crates/ui/src/window_link_clipboard.rs`
- bare-URL detector reuse: `crates/decorate/src/autolink.rs` (Phase B12)

## Relates to
- [Selections + edits](selection-edits.md) — paste dispatches `SelectionEdit::InsertText` or `SurroundSelection`.
- [Decoration](decoration.md) — RTF copy uses block + inline style spans.
- [File I/O](file-io.md) — RTF / encoding logic stays separate from disk I/O (clipboard never blocks on disk).
