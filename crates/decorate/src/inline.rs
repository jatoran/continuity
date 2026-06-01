//! Inline-span extraction within a markdown block.
//!
//! `tree-sitter-md` ships a block grammar; inline content (emphasis, code
//! spans, links, image refs, inline checkboxes) is left to consumers. We
//! parse those inline shapes by hand against the block's source slice.
//!
//! Pure function: input is a block's source text + its absolute byte offset
//! in the document; output is a `Vec<InlineSpan>` whose byte ranges are
//! absolute (document-relative) so renderer/cursor logic can compare them
//! directly with rope offsets.
//!
//! **Thread ownership**: caller-side. Used both on the UI thread (cheap
//! re-parses for short blocks) and on decoration worker threads.

use crate::inline_text::scan_text_inlines;
use crate::BlockKind;

/// What kind of inline span this is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineKind {
    /// Bold (`**…**` or `__…__`).
    Strong,
    /// Italic (`*…*` or `_…_`).
    Emphasis,
    /// Strikethrough (`~~…~~`).
    Strikethrough,
    /// Inline code (`` `…` ``).
    Code,
    /// `[text](url)` link.
    Link {
        /// Absolute byte range of the link's display text (the part inside `[...]`).
        text_range: ByteRange,
        /// Absolute byte range of the URL (the part inside `(...)`).
        url_range: ByteRange,
    },
    /// `[^label]` footnote reference in body text.
    FootnoteReference {
        /// Label text without the `[^` / `]` delimiters.
        label: String,
    },
    /// `[^label]: body` footnote definition label.
    FootnoteDefinition {
        /// Label text without the `[^` / `]` delimiters.
        label: String,
        /// Absolute byte range of the definition body.
        body_range: ByteRange,
    },
    /// `![alt](url)` image reference.
    ImageRef {
        /// Absolute byte range of the alt text (inside `[...]`).
        alt_range: ByteRange,
        /// Absolute byte range of the URL (inside `(...)`).
        url_range: ByteRange,
    },
    /// `[ ]` or `[x]` task-list checkbox at the start of a list item.
    Checkbox {
        /// `true` for `[x]` / `[X]`; `false` for `[ ]`.
        checked: bool,
        /// Byte offset of the single source character that toggles state
        /// (the space/`x` between brackets).
        toggle_byte: usize,
    },
    /// A purely structural marker that is hidden when no caret intersects
    /// the enclosing block (heading hashes, list markers, fence ticks,
    /// emphasis/strike/code delimiters, table pipes, etc.).
    Marker(MarkerKind),
}

/// What kind of structural marker — used to choose cursor-skip behavior in
/// the UI layer (purely structural skips in one arrow press; emphasis takes
/// an extra step per spec §9).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MarkerKind {
    /// `#` … `######` heading prefix.
    HeadingHash,
    /// `- `, `* `, `+ `, `1. ` list bullet.
    ListMarker,
    /// ``` ``` ``` code-fence ticks (opening or closing).
    FenceTick,
    /// `>` blockquote marker.
    BlockquoteCaret,
    /// `*`/`**`/`_`/`__` emphasis delimiter.
    EmphasisDelim,
    /// `~~` strike delimiter.
    StrikeDelim,
    /// `` ` `` inline-code delimiter.
    CodeDelim,
    /// `|` pipe-table column separator.
    TablePipe,
    /// `---` thematic break.
    ThematicBreak,
}

/// Inclusive-start, exclusive-end byte range. Document-absolute.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct ByteRange {
    /// Inclusive start byte.
    pub start: usize,
    /// Exclusive end byte.
    pub end: usize,
}

impl ByteRange {
    /// Construct from explicit endpoints.
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// `true` when `byte` falls inside `[start, end)`.
    #[must_use]
    pub fn contains(&self, byte: usize) -> bool {
        byte >= self.start && byte < self.end
    }

    /// `true` when this range's start equals its end.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// One inline span within a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineSpan {
    /// What kind of inline span.
    pub kind: InlineKind,
    /// Byte range covered (document-absolute).
    pub range: ByteRange,
}

/// Parse the inline structure of a single block at `block_start_byte` whose
/// source is `block_src` (newlines included). Returns spans in source order
/// with document-absolute byte ranges.
///
/// `block_kind` selects which inline shapes are looked for — for example
/// fenced code blocks contribute fence-tick markers but no emphasis.
#[must_use]
pub fn block_inline_spans(
    block_kind: BlockKind,
    block_start_byte: usize,
    block_src: &str,
) -> Vec<InlineSpan> {
    let mut out = Vec::new();
    match block_kind {
        BlockKind::Heading { .. } => {
            scan_heading_marker(block_start_byte, block_src, &mut out);
            scan_text_inlines(block_start_byte, block_src, &mut out);
        }
        BlockKind::SetextHeading { .. } => {
            scan_text_inlines(block_start_byte, block_src, &mut out);
        }
        BlockKind::Paragraph => {
            // Each line of a paragraph may start with what looks like a
            // bullet marker (`- foo`, `  * bar`) even though the
            // CommonMark parser absorbed it into the paragraph because
            // no list context was open above. The user-visible
            // expectation in a notes editor is still a bullet glyph on
            // those lines, so scan every line for the pattern.
            scan_per_line_list_markers(block_start_byte, block_src, &mut out, false);
            scan_text_inlines(block_start_byte, block_src, &mut out);
        }
        BlockKind::ListItem => {
            scan_list_marker_and_checkbox(block_start_byte, block_src, &mut out);
            // Continuation lines inside a single ListItem block (the
            // parser keeps lazy-continuation content under the same
            // item) can also look like nested bullets — pick those up
            // on every line *after* the first, since the first is
            // already handled by `scan_list_marker_and_checkbox`.
            scan_per_line_list_markers(block_start_byte, block_src, &mut out, true);
            scan_text_inlines(block_start_byte, block_src, &mut out);
        }
        BlockKind::BlockQuote => {
            scan_blockquote_markers(block_start_byte, block_src, &mut out);
            scan_text_inlines(block_start_byte, block_src, &mut out);
        }
        BlockKind::FencedCodeBlock => {
            scan_fence_markers(block_start_byte, block_src, &mut out);
        }
        BlockKind::HorizontalRule => {
            scan_thematic_break(block_start_byte, block_src, &mut out);
        }
        BlockKind::PipeTable => {
            scan_pipe_table(block_start_byte, block_src, &mut out);
        }
        // List wrapper, indented code, html, other — no per-block inlines beyond
        // structural recursion handled by caller.
        _ => {}
    }
    out.sort_by_key(|s| s.range.start);
    out
}

fn scan_heading_marker(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    let bytes = src.as_bytes();
    if bytes.is_empty() {
        return;
    }
    // Optional leading whitespace per CommonMark.
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let mark_start = i;
    while i < bytes.len() && bytes[i] == b'#' {
        i += 1;
    }
    if i == mark_start {
        return;
    }
    // Hashes plus the trailing space (if any).
    let mut end = i;
    if end < bytes.len() && bytes[end] == b' ' {
        end += 1;
    }
    out.push(InlineSpan {
        kind: InlineKind::Marker(MarkerKind::HeadingHash),
        range: ByteRange::new(base + mark_start, base + end),
    });
}

/// Emit a `ListMarker` span for every line in `src` whose leading
/// whitespace is followed by a `-` / `*` / `+ ` glyph or an ordered
/// marker (`123. ` / `123) `). Used to give "looks like a bullet"
/// lines the same `• ` glyph treatment even when the CommonMark
/// parser folded them into a paragraph (no parent list context) or
/// kept them inside the same ListItem block as continuation content.
///
/// `skip_first_line` lets the ListItem caller avoid double-emitting
/// the marker that `scan_list_marker_and_checkbox` already produced
/// for the item's opening line.
fn scan_per_line_list_markers(base: usize, src: &str, out: &mut Vec<InlineSpan>, skip_first: bool) {
    let bytes = src.as_bytes();
    let mut line_start = 0usize;
    let mut line_idx: usize = 0;
    while line_start <= bytes.len() {
        let mut line_end = line_start;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let is_first = line_idx == 0;
        line_idx += 1;
        if !(skip_first && is_first) {
            // Reuse the per-line matching logic on this line slice.
            scan_one_line_list_marker(base + line_start, &bytes[line_start..line_end], out);
        }
        if line_end >= bytes.len() {
            break;
        }
        line_start = line_end + 1;
    }
}

/// Match a single line's leading list marker (no checkbox handling —
/// checkboxes are only meaningful on the item's opening line, which
/// `scan_list_marker_and_checkbox` already covers).
fn scan_one_line_list_marker(base: usize, bytes: &[u8], out: &mut Vec<InlineSpan>) {
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let mark_start = i;
    let mut end = mark_start;
    if end >= bytes.len() {
        return;
    }
    match bytes[end] {
        b'-' | b'*' | b'+' => {
            end += 1;
        }
        b'0'..=b'9' => {
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end < bytes.len() && (bytes[end] == b'.' || bytes[end] == b')') {
                end += 1;
            } else {
                return;
            }
        }
        _ => return,
    }
    if end < bytes.len() && bytes[end] == b' ' {
        end += 1;
    } else {
        return;
    }
    out.push(InlineSpan {
        kind: InlineKind::Marker(MarkerKind::ListMarker),
        range: ByteRange::new(base + mark_start, base + end),
    });
}

fn scan_list_marker_and_checkbox(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    // Match optional indent then `[-*+] ` or `\d+[.)] `.
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let mark_start = i;
    let mut end = mark_start;
    if end < bytes.len() {
        match bytes[end] {
            b'-' | b'*' | b'+' => {
                end += 1;
            }
            b'0'..=b'9' => {
                while end < bytes.len() && bytes[end].is_ascii_digit() {
                    end += 1;
                }
                if end < bytes.len() && (bytes[end] == b'.' || bytes[end] == b')') {
                    end += 1;
                } else {
                    return;
                }
            }
            _ => return,
        }
    } else {
        return;
    }
    if end < bytes.len() && bytes[end] == b' ' {
        end += 1;
    } else {
        return;
    }
    // Inline checkbox `[ ]` / `[x]` immediately after the marker. When
    // present, the whole `- [ ] ` is one task prefix: emit a *single*
    // Checkbox span covering the marker **and** the brackets (plus the
    // one trailing space) so the display map replaces the entire prefix
    // with a checkbox glyph. The list bullet is intentionally not emitted
    // here — otherwise the line renders a bullet *and* a checkbox.
    if end + 3 <= bytes.len() && bytes[end] == b'[' && bytes[end + 2] == b']' {
        let inner = bytes[end + 1];
        let checked = matches!(inner, b'x' | b'X');
        let unchecked = inner == b' ';
        if checked || unchecked {
            let mut box_end = end + 3;
            if box_end < bytes.len() && bytes[box_end] == b' ' {
                box_end += 1;
            }
            out.push(InlineSpan {
                kind: InlineKind::Checkbox {
                    checked,
                    toggle_byte: base + end + 1,
                },
                range: ByteRange::new(base + mark_start, base + box_end),
            });
            return;
        }
    }
    // Plain (non-task) list item: emit the bullet marker.
    out.push(InlineSpan {
        kind: InlineKind::Marker(MarkerKind::ListMarker),
        range: ByteRange::new(base + mark_start, base + end),
    });
}

fn scan_blockquote_markers(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    // Each line in the block may begin with `> ` (or just `>`).
    let mut line_start = 0usize;
    let bytes = src.as_bytes();
    while line_start < bytes.len() {
        let mut i = line_start;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'>' {
            let mark_start = i;
            i += 1;
            if i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            out.push(InlineSpan {
                kind: InlineKind::Marker(MarkerKind::BlockquoteCaret),
                range: ByteRange::new(base + mark_start, base + i),
            });
        }
        // Advance to next line.
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        if i < bytes.len() {
            i += 1;
        }
        line_start = i;
    }
}

fn scan_fence_markers(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    // First and last line of a fenced code block are the fence lines; mark
    // them as FenceTick markers (info string included on the opening line).
    let bytes = src.as_bytes();
    if bytes.is_empty() {
        return;
    }
    // Find end of first line.
    let mut first_end = 0usize;
    while first_end < bytes.len() && bytes[first_end] != b'\n' {
        first_end += 1;
    }
    out.push(InlineSpan {
        kind: InlineKind::Marker(MarkerKind::FenceTick),
        range: ByteRange::new(base, base + first_end),
    });
    // Find start of last non-empty line.
    let mut tail = bytes.len();
    while tail > 0 && (bytes[tail - 1] == b'\n' || bytes[tail - 1] == b'\r') {
        tail -= 1;
    }
    let mut last_start = tail;
    while last_start > 0 && bytes[last_start - 1] != b'\n' {
        last_start -= 1;
    }
    let last_line = &bytes[last_start..tail];
    if last_line.iter().all(|b| matches!(*b, b'`' | b'~' | b' ')) && last_start >= first_end {
        out.push(InlineSpan {
            kind: InlineKind::Marker(MarkerKind::FenceTick),
            range: ByteRange::new(base + last_start, base + tail),
        });
    }
}

fn scan_thematic_break(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    // Cover the whole block as a thematic-break marker for paint logic.
    out.push(InlineSpan {
        kind: InlineKind::Marker(MarkerKind::ThematicBreak),
        range: ByteRange::new(base, base + src.len()),
    });
}

fn scan_pipe_table(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    // Walk each line; mark every `|` byte as a TablePipe marker. The
    // delimiter row (`---` / `:---:`) also gets covered.
    let bytes = src.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'|' {
            out.push(InlineSpan {
                kind: InlineKind::Marker(MarkerKind::TablePipe),
                range: ByteRange::new(base + i, base + i + 1),
            });
        }
    }
}

#[cfg(test)]
mod tests;
