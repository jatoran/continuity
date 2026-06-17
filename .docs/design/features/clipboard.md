# Clipboard

Cut, copy, and paste via `CF_UNICODETEXT`. Paste also reads `CF_HTML` ("HTML Format") and converts pasted rich-text/browser HTML to markdown. Smart-paste rewrites a clipboard URL into a markdown link / image / autolink based on context. An in-memory paste-history ring lets the user reach back N entries; nothing is persisted to disk.

## What it is
- Copy / cut / paste over `CF_UNICODETEXT`. RTF copy (`Ctrl+Shift+Alt+C`) for Word / Outlook compatibility. Rich paste (Phase D item 16) converts a clipboard `CF_HTML` fragment to markdown. Smart paste (Phase B13) transforms a clipboard URL into a markdown link / image / autolink. In-memory paste history ring (no persistence per spec §12).

## Key concepts
- **`clipboard::read_text(hwnd)`** — opens the clipboard, reads `CF_UNICODETEXT`, normalizes line endings.
- **`clipboard::write_text(hwnd, &str)`** — opens the clipboard, writes `CF_UNICODETEXT`. Always normalize `\r\n` → `\n` on read; writers emit `\r\n` since Windows consumers expect it.
- **`clipboard::has_html()` / `clipboard::read_html(hwnd)`** — query / read the `"HTML Format"` payload (registered once via `RegisterClipboardFormatW`, cached in a `OnceLock`). `read_html` extracts only the `StartFragment..EndFragment` slice from the CF_HTML ASCII header (`extract_html_fragment`), so only the bytes the source app actually copied are converted. Returns `None` when no HTML format is advertised, the header is malformed, the offsets are out of range, or the fragment is empty (then the caller falls back to body-after-header, then to plain text).
- **`html_to_markdown::html_to_markdown(fragment) -> Option<String>`** — dependency-free HTML→markdown converter (`crates/ui/src/html_to_markdown.rs`). Parses the fragment with a hand-rolled tokenizer + forgiving stack tree builder (`crates/ui/src/clipboard_html.rs`), walks the DOM, and renders markdown. Element coverage: `a`→`[text](href)`, `img`→`![alt](src)`, `b`/`strong`→`**`, `i`/`em`→`*`, `s`/`del`/`strike`→`~~`, inline `code`→`` ` ``, `pre`(+`pre>code`)→fenced block, `h1`–`h6`→`#`..`######`, `ul`/`ol`/`li`→nested lists, `blockquote`→`> `, `br`→hard break, `p`/`div`→blank-line-separated blocks, `table`/`tr`/`th`/`td`→GFM pipe table (`clipboard_html/table.rs`). `script`/`style` bodies discarded; unmodeled containers render their children transparently (no text dropped). Browser whitespace collapse; markdown-significant chars escaped in prose; `None` when no usable content.
- **`PasteHistory`** — bounded ring (16 entries) of recently-copied text. Newest first; dedup on push. Not persisted.
- **RTF copy** — `crates/ui/src/window_link_clipboard.rs::put_clipboard_html` (variant). Builds a minimal RTF stream from the selection's `Decorations` styles.
- **Smart paste** — `crates/ui/src/smart_paste.rs::smart_paste_transform(clipboard_text, has_selection) -> Option<SmartPasteOp>`:
  - selection non-empty + URL → `SurroundSelection { open: "[", close: "](url)" }`
  - no selection + image extension URL → `InsertText("![](url)")`
  - no selection + plain URL → `InsertText("<url>")`
  - otherwise → `None` (fall through to plain paste)

## Operations

### Copy / cut
- `editor.copy` — copies every non-collapsed selection's source text in document order, joined by `\n`. With one selection that is just that selection's text; with Ctrl-drag multi-region highlights the clipboard carries all of them (`window_clipboard::selections_clipboard_source`).
- `editor.cut` — same multi-region gather, then deletes every selected range in one undo group (`dispatch_selection_edit(SelectionEdit::InsertText(""))`). Copy and cut therefore put the *same* text on the clipboard that cut removes.
- `editor.copy_as_rtf` (`Ctrl+Shift+Alt+C`) — copies as styled text with `markdown.*` styles baked in.

### Paste (`Ctrl+V`, `editor.paste`)
`paste_clipboard_impl` resolves the clipboard in priority order — first match wins, the rest are skipped:
1. **Image** — `try_paste_clipboard_image` (CF_DIB / image formats). Consuming an image returns early so a screenshot doesn't also paste any text alternate the source app populated alongside it.
2. **CF_HTML** — when `clipboard::has_html()`, read the fragment and run `html_to_markdown`. On a usable conversion, normalize line endings and insert the markdown (via `insert_paste_text`, which applies table-block normalization — see [Tables](tables.md)). An unusable/empty HTML fragment falls through to plain text.
3. **Smart-paste URL** — on the `CF_UNICODETEXT` payload, `smart_paste_transform` decides if this is a URL/image/link transform.
4. **Plain text** — otherwise insert the normalized `CF_UNICODETEXT` payload (also via `insert_paste_text`).

All text inserts dispatch a single `SelectionEdit` undo group and arm the edit-region pulse.

### Plain paste (`Ctrl+Shift+V`, `editor.paste_as_plain_text`)
- **Bypasses both the image and CF_HTML branches** as well as smart-paste: routes through `insert_plain_clipboard_text`, which reads `CF_UNICODETEXT` only, normalizes line endings, and inserts verbatim. The HTML→markdown path and the table-block normalization are NOT applied. When the clipboard holds an image but no text, this is a no-op (plain paste never imports images).

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
- Reader / writer: `crates/win/src/clipboard.rs::{read_text, write_text, has_text, has_html, read_html, Error}`.
- HTML→markdown converter: `crates/ui/src/html_to_markdown.rs::html_to_markdown`; HTML parser `crates/ui/src/clipboard_html.rs::{parse_html, HtmlNode}`; GFM table rendering `crates/ui/src/clipboard_html/table.rs::render_table`; block renderers `crates/ui/src/html_to_markdown/blocks.rs`.
- Paste history: `crates/ui/src/window_clipboard.rs::PasteHistory`.
- Smart paste: `crates/ui/src/smart_paste.rs::{smart_paste_transform, SmartPasteOp}`.
- Window handlers: `crates/ui/src/window_clipboard.rs::{copy_selection_impl, cut_selection_impl, selections_clipboard_source, paste_clipboard_impl, paste_as_plain_text_impl, insert_paste_text, insert_plain_clipboard_text, normalize_table_paste, paste_from_history_impl, copy_caret_line_impl}` + rich-copy variants in `crates/ui/src/window_link_clipboard.rs`.

## Configuration
- `clipboard.history_size` (default `16`).
- `clipboard.smart_paste_url` (default `true`).
- `clipboard.bare_url_as_autolink` (default `true` — false uses plain `<url>` form; only matters when no selection + non-image URL).

## Key files
- Win32 clipboard wrapper (CF_UNICODETEXT + CF_HTML read): `crates/win/src/clipboard.rs`
- copy / cut / paste / paste-history ring: `crates/ui/src/window_clipboard.rs`
- HTML→markdown paste converter: `crates/ui/src/html_to_markdown.rs`, `crates/ui/src/html_to_markdown/blocks.rs`
- dependency-free HTML parser: `crates/ui/src/clipboard_html.rs`, `crates/ui/src/clipboard_html/table.rs`
- pasted-table block normalization: `crates/ui/src/window_markdown_table_ops/paste_normalize.rs` (see [Tables](tables.md))
- smart paste: `crates/ui/src/smart_paste.rs`
- rich-copy variants (rendered / source / HTML): `crates/ui/src/window_link_clipboard.rs`
- bare-URL detector reuse: `crates/decorate/src/autolink.rs` (Phase B12)

## Relates to
- [Selections + edits](selection-edits.md) — paste dispatches `SelectionEdit::InsertText` or `SurroundSelection`.
- [Decoration](decoration.md) — RTF copy uses block + inline style spans.
- [File I/O](file-io.md) — RTF / encoding logic stays separate from disk I/O (clipboard never blocks on disk).
