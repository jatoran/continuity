//! `Decorations` — the snapshot of all visual decoration data computed for
//! one `(RopeSnapshot, Revision)` pair.
//!
//! **Thread ownership**: produced on a decoration worker thread, transferred
//! by value (`Send`) into a UI-side cache. Once accepted, the UI thread is
//! the sole reader.
//!
//! Stale results (revision mismatch) are discarded by the consumer per spec
//! §2 — `Decorations` carries the revision it was computed against so the
//! consumer can compare against the current `Buffer::revision()` cheaply.

use crate::footnotes::footnote_definition_spans;
use crate::inline::{block_inline_spans, ByteRange, InlineKind, InlineSpan, MarkerKind};
use crate::inline_color::{inline_color_spans, InlineColorSpan};
use crate::spans::{block_spans, BlockKind, BlockSpan};
use crate::syntax::{highlight, HighlightSpan};
use crate::table_block_fixup::fill_empty_pipe_rows_for_parser;
use crate::table_eval::{evaluate_tables, EvaluatedTable};
use crate::MarkdownParser;
use tree_sitter::Tree;

/// Recursively scan a block's source for inline spans. The block-grammar
/// `block_spans` collector stops at top-level children, but lists wrap
/// list items which themselves wrap paragraphs — to surface checkboxes
/// and emphasis within list bodies we descend into list shapes ourselves
/// by re-invoking `block_inline_spans` per logical sub-line.
fn collect_block_inlines(kind: BlockKind, base: usize, block_src: &str, out: &mut Vec<InlineSpan>) {
    match kind {
        BlockKind::List => {
            // A list contains list items; scan each line that begins with a
            // list marker as a `ListItem`, plus do the inline text scan
            // for the rest of the line.
            for (line_offset, line) in line_spans(block_src) {
                let mut sub = block_inline_spans(BlockKind::ListItem, base + line_offset, line);
                out.append(&mut sub);
            }
        }
        BlockKind::ListItem => {
            // ListItem's first line carries the marker; subsequent lines
            // are continuation prose. Scan the first line as ListItem,
            // remaining lines as Paragraph.
            let mut iter = line_spans(block_src).into_iter();
            if let Some((off, first)) = iter.next() {
                let mut sub = block_inline_spans(BlockKind::ListItem, base + off, first);
                out.append(&mut sub);
            }
            for (off, line) in iter {
                let mut sub = block_inline_spans(BlockKind::Paragraph, base + off, line);
                out.append(&mut sub);
            }
        }
        _ => {
            let mut sub = block_inline_spans(kind, base, block_src);
            out.append(&mut sub);
        }
    }
}

/// Split `src` into `(byte_offset, line_without_trailing_newline)` pairs.
fn line_spans(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let line = &src[start..i];
        out.push((start, line));
        if i < bytes.len() {
            i += 1;
        }
    }
    out
}

/// One decoration snapshot — block + inline spans for a buffer at a known
/// revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decorations {
    /// Buffer revision this snapshot was computed against. Consumers must
    /// discard the snapshot when this differs from the current buffer
    /// revision.
    pub revision: u64,
    /// Block-level structural spans (from tree-sitter-md).
    pub blocks: Vec<BlockSpan>,
    /// Inline-level spans (emphasis, code, links, image refs, checkboxes,
    /// markers). Document-absolute byte ranges; sorted by `range.start`.
    pub inlines: Vec<InlineSpan>,
    /// Syntax-highlight spans for fenced code-block bodies or whole-file
    /// code buffers. Document-absolute byte ranges; sorted by `start`.
    pub highlights: Vec<HighlightSpan>,
    /// Phase F3 — inline color / highlight spans (`==…==` and
    /// `{#hex:…}`). Document order; non-overlapping; never cross newlines.
    /// Empty for a buffer with no inline color markup.
    pub inline_color_spans: Vec<InlineColorSpan>,
    /// Phase F4 — per-revision formula-evaluation overrides for pipe-
    /// table blocks. Each entry covers one block; `overrides` is empty
    /// for tables with no formula cells. Document order.
    pub evaluated_tables: Vec<EvaluatedTable>,
}

impl Decorations {
    /// Empty placeholder for a freshly-opened buffer where no decoration has
    /// yet been computed.
    #[must_use]
    pub fn empty(revision: u64) -> Self {
        Self {
            revision,
            blocks: Vec::new(),
            inlines: Vec::new(),
            highlights: Vec::new(),
            inline_color_spans: Vec::new(),
            evaluated_tables: Vec::new(),
        }
    }

    /// Compute syntax-only decorations for a non-markdown code buffer.
    #[must_use]
    pub fn compute_code(source: &str, revision: u64, language_tag: &str) -> Self {
        Self {
            revision,
            blocks: Vec::new(),
            inlines: Vec::new(),
            highlights: highlight(language_tag, source),
            inline_color_spans: Vec::new(),
            evaluated_tables: Vec::new(),
        }
    }

    /// Compute decorations for `source` at `revision` synchronously.
    ///
    /// Returns `None` when the parser fails to construct (only on a tree-
    /// sitter ABI mismatch — bundled grammar is expected to load).
    #[must_use]
    pub fn compute(source: &str, revision: u64) -> Option<Self> {
        Self::compute_with_tree(source, revision).map(|(decorations, _tree)| decorations)
    }

    /// Compute decorations and return the parse tree used to derive them
    /// alongside split parse / extract timings in microseconds.
    /// `tree_query_us` covers parser construction + tree-sitter parse;
    /// `decoration_compute_us` covers the span extraction
    /// [`Self::from_tree`] that runs after the tree is available.
    #[must_use]
    pub fn compute_with_tree_split(source: &str, revision: u64) -> Option<(Self, Tree, u64, u64)> {
        let parse_started = std::time::Instant::now();
        let mut parser = MarkdownParser::new().ok()?;
        let parse_owned = fill_empty_pipe_rows_for_parser(source);
        let parse_str: &str = parse_owned.as_deref().unwrap_or(source);
        let tree = parser.parse(parse_str, None)?;
        let tree_query_us = u64::try_from(parse_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let extract_started = std::time::Instant::now();
        let decorations = Self::from_tree(source, revision, &tree);
        let decoration_compute_us =
            u64::try_from(extract_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        Some((decorations, tree, tree_query_us, decoration_compute_us))
    }

    /// Compute decorations and return the parse tree used to derive them.
    ///
    /// The worker cache uses this for the initial full parse of a buffer,
    /// then feeds the returned tree into [`Self::compute_incremental`] for
    /// later revisions.
    #[must_use]
    pub fn compute_with_tree(source: &str, revision: u64) -> Option<(Self, Tree)> {
        let mut parser = MarkdownParser::new().ok()?;
        // Tree-sitter-md's pipe-table grammar truncates the table at
        // the last row with non-whitespace cell content. A
        // `format_table_skeleton(rows, cols)` skeleton therefore parses
        // as a header+alignment-only PipeTable, and every line below
        // — both the empty body rows AND any user-added bullets /
        // images / paragraphs — gets lumped into a single
        // `Other("unknown")` block, robbing the downstream markdown of
        // its block classification. Pre-fill empty cells with a single
        // non-whitespace byte before parsing so tree-sitter recognises
        // the full table and the lines below are free to be classified
        // as List / Heading / etc. Substitution is byte-for-byte so
        // block / inline byte ranges still match the original rope.
        let parse_owned = fill_empty_pipe_rows_for_parser(source);
        let parse_str: &str = parse_owned.as_deref().unwrap_or(source);
        let tree = parser.parse(parse_str, None)?;
        let decorations = Self::from_tree(source, revision, &tree);
        Some((decorations, tree))
    }

    /// Extract decorations from an already parsed markdown tree.
    pub(crate) fn from_tree(source: &str, revision: u64, tree: &Tree) -> Self {
        let blocks = block_spans(tree);
        let mut inlines = Vec::new();
        for span in &blocks {
            // Inline scanning walks the *original* rope bytes (so
            // empty cells remain empty for downstream rendering /
            // export), with byte ranges shared between the parse
            // source and the rope.
            let block_src = source.get(span.start_byte..span.end_byte).unwrap_or("");
            collect_block_inlines(span.kind, span.start_byte, block_src, &mut inlines);
        }
        inlines.extend(footnote_definition_spans(source));
        inlines.sort_by_key(|s| s.range.start);
        let highlights = fenced_code_highlights(source, &blocks);
        let inline_color_spans = inline_color_spans(source);
        let evaluated_tables = evaluate_tables(source, &blocks);
        Self {
            revision,
            blocks,
            inlines,
            highlights,
            inline_color_spans,
            evaluated_tables,
        }
    }

    /// Find the inline-color span whose `outer` byte range contains
    /// `byte`. Returns `None` when no span covers the byte. The renderer
    /// uses this for caret-inside-span reveal; the
    /// `markdown.clear_inline_color` handler uses it to locate the span
    /// to unwrap.
    #[must_use]
    pub fn inline_color_span_at(&self, byte: usize) -> Option<&InlineColorSpan> {
        self.inline_color_spans
            .iter()
            .find(|s| byte >= s.outer.start && byte < s.outer.end)
    }

    /// `true` when any caret byte falls inside a pipe-table block. Mirrors
    /// the per-block reveal rule used for `MarkerKind::TablePipe` markers.
    #[must_use]
    pub fn caret_inside_any_table_block(&self, caret_bytes: &[usize]) -> bool {
        self.evaluated_tables.iter().any(|t| {
            caret_bytes
                .iter()
                .any(|c| *c >= t.block_range.start && *c < t.block_range.end)
        })
    }

    /// Find the block whose byte range covers `byte`. Returns `None` when
    /// no block spans the position (e.g. byte past EOF).
    #[must_use]
    pub fn block_at(&self, byte: usize) -> Option<&BlockSpan> {
        self.blocks
            .iter()
            .find(|b| byte >= b.start_byte && byte < b.end_byte)
    }

    /// `true` iff any block in `self.blocks` overlaps `byte`.
    #[must_use]
    pub fn intersects_block(&self, block: &BlockSpan, byte: usize) -> bool {
        byte >= block.start_byte && byte < block.end_byte
    }

    /// Iterator over inline spans whose ranges overlap `[start, end)`.
    pub fn inlines_in(&self, start: usize, end: usize) -> impl Iterator<Item = &InlineSpan> {
        self.inlines
            .iter()
            .filter(move |s| s.range.end > start && s.range.start < end)
    }

    /// Compute the byte ranges that should be visually hidden when no caret
    /// intersects their enclosing block.
    ///
    /// `caret_bytes` is the set of caret byte positions (one per selection
    /// head). A block is "revealed" — its markers visible — when any caret
    /// falls inside `[block.start_byte, block.end_byte)`.
    #[must_use]
    pub fn hidden_marker_ranges(&self, caret_bytes: &[usize]) -> Vec<ByteRange> {
        let mut out = Vec::new();
        for block in &self.blocks {
            let revealed = caret_bytes
                .iter()
                .any(|c| *c >= block.start_byte && *c < block.end_byte);
            if revealed {
                continue;
            }
            for span in self
                .inlines
                .iter()
                .filter(|s| s.range.start >= block.start_byte && s.range.end <= block.end_byte)
            {
                if matches!(span.kind, InlineKind::Marker(_)) {
                    out.push(span.range);
                }
            }
        }
        out
    }

    /// `true` if `byte` falls within an inline span whose marker classifies
    /// as purely structural (cursor should skip past in one arrow press).
    #[must_use]
    pub fn is_structural_marker_byte(&self, byte: usize) -> bool {
        self.inlines.iter().any(|s| {
            s.range.contains(byte)
                && matches!(
                    s.kind,
                    InlineKind::Marker(
                        MarkerKind::HeadingHash
                            | MarkerKind::ListMarker
                            | MarkerKind::FenceTick
                            | MarkerKind::BlockquoteCaret
                            | MarkerKind::TablePipe
                            | MarkerKind::ThematicBreak,
                    )
                )
        })
    }

    /// `true` if `byte` falls within an emphasis/strike/code delimiter —
    /// arrow keys step into these but require a second press to cross out.
    #[must_use]
    pub fn is_emphasis_marker_byte(&self, byte: usize) -> bool {
        self.inlines.iter().any(|s| {
            s.range.contains(byte)
                && matches!(
                    s.kind,
                    InlineKind::Marker(
                        MarkerKind::EmphasisDelim | MarkerKind::StrikeDelim | MarkerKind::CodeDelim,
                    )
                )
        })
    }

    /// Locate the checkbox span whose hit-rect contains `byte`. Returns
    /// `(checked, toggle_byte)` for the renderer to emit a click handler.
    #[must_use]
    pub fn checkbox_at(&self, byte: usize) -> Option<(bool, usize)> {
        self.inlines.iter().find_map(|s| {
            if s.range.contains(byte) {
                if let InlineKind::Checkbox {
                    checked,
                    toggle_byte,
                } = s.kind
                {
                    return Some((checked, toggle_byte));
                }
            }
            None
        })
    }

    /// Locate the URL byte range of any link/image whose display range
    /// contains `byte` (used for Ctrl+click open).
    #[must_use]
    pub fn url_at(&self, byte: usize) -> Option<ByteRange> {
        self.inlines.iter().find_map(|s| {
            if !s.range.contains(byte) {
                return None;
            }
            match &s.kind {
                InlineKind::Link { url_range, .. } | InlineKind::ImageRef { url_range, .. } => {
                    Some(*url_range)
                }
                _ => None,
            }
        })
    }

    /// Locate the footnote reference containing `byte`.
    #[must_use]
    pub fn footnote_reference_at(&self, byte: usize) -> Option<(String, ByteRange)> {
        self.inlines.iter().find_map(|s| {
            if !s.range.contains(byte) {
                return None;
            }
            match &s.kind {
                InlineKind::FootnoteReference { label } => Some((label.clone(), s.range)),
                _ => None,
            }
        })
    }

    /// Locate the footnote definition label containing `byte`.
    #[must_use]
    pub fn footnote_definition_at(&self, byte: usize) -> Option<(String, ByteRange, ByteRange)> {
        self.inlines.iter().find_map(|s| {
            if !s.range.contains(byte) {
                return None;
            }
            match &s.kind {
                InlineKind::FootnoteDefinition { label, body_range } => {
                    Some((label.clone(), s.range, *body_range))
                }
                _ => None,
            }
        })
    }

    /// Find the definition label and body ranges for `label`.
    #[must_use]
    pub fn footnote_definition_for(&self, label: &str) -> Option<(ByteRange, ByteRange)> {
        self.inlines.iter().find_map(|s| match &s.kind {
            InlineKind::FootnoteDefinition {
                label: candidate,
                body_range,
            } if candidate == label => Some((s.range, *body_range)),
            _ => None,
        })
    }

    /// Find the first body-text reference range for `label`.
    #[must_use]
    pub fn footnote_first_reference_for(&self, label: &str) -> Option<ByteRange> {
        self.inlines.iter().find_map(|s| match &s.kind {
            InlineKind::FootnoteReference { label: candidate } if candidate == label => {
                Some(s.range)
            }
            _ => None,
        })
    }
}

fn fenced_code_highlights(source: &str, blocks: &[BlockSpan]) -> Vec<HighlightSpan> {
    let mut out = Vec::new();
    for block in blocks
        .iter()
        .filter(|block| matches!(block.kind, BlockKind::FencedCodeBlock))
    {
        let Some(block_src) = source.get(block.start_byte..block.end_byte) else {
            continue;
        };
        let Some((language_tag, body_start, body_end)) = fenced_code_body(block_src) else {
            continue;
        };
        let Some(body) = block_src.get(body_start..body_end) else {
            continue;
        };
        out.extend(
            highlight(language_tag, body)
                .into_iter()
                .map(|span| HighlightSpan {
                    start: block.start_byte + body_start + span.start,
                    end: block.start_byte + body_start + span.end,
                    kind: span.kind,
                }),
        );
    }
    out.sort_by_key(|span| span.start);
    out
}

fn fenced_code_body(block_src: &str) -> Option<(&str, usize, usize)> {
    let bytes = block_src.as_bytes();
    let mut first_end = 0usize;
    while first_end < bytes.len() && bytes[first_end] != b'\n' {
        first_end += 1;
    }
    let opener = &block_src[..first_end];
    let language_tag = fence_language_tag(opener)?;
    let body_start = if first_end < bytes.len() {
        first_end + 1
    } else {
        bytes.len()
    };
    let body_end = closing_fence_start(block_src, body_start).unwrap_or(bytes.len());
    Some((language_tag, body_start, body_end))
}

fn fence_language_tag(opener: &str) -> Option<&str> {
    let trimmed = opener.trim_start();
    let bytes = trimmed.as_bytes();
    let fence = *bytes.first()?;
    if fence != b'`' && fence != b'~' {
        return None;
    }
    let mut i = 0usize;
    while i < bytes.len() && bytes[i] == fence {
        i += 1;
    }
    if i < 3 {
        return None;
    }
    Some(trimmed[i..].trim())
}

fn closing_fence_start(block_src: &str, body_start: usize) -> Option<usize> {
    let bytes = block_src.as_bytes();
    let mut tail = bytes.len();
    while tail > body_start && matches!(bytes[tail - 1], b'\n' | b'\r') {
        tail -= 1;
    }
    let mut last_start = tail;
    while last_start > body_start && bytes[last_start - 1] != b'\n' {
        last_start -= 1;
    }
    let last_line = &bytes[last_start..tail];
    let mut i = 0usize;
    while i < last_line.len() && matches!(last_line[i], b' ' | b'\t') {
        i += 1;
    }
    let fence = last_line.get(i).copied()?;
    if fence != b'`' && fence != b'~' {
        return None;
    }
    let fence_start = i;
    while i < last_line.len() && last_line[i] == fence {
        i += 1;
    }
    if i - fence_start >= 3 && last_line[i..].iter().all(|b| matches!(*b, b' ' | b'\t')) {
        return Some(last_start);
    }
    None
}

#[cfg(test)]
mod tests;
