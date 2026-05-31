//! Extract a flat hierarchical list of markdown headings for goto-heading.
//!
//! Pure function: `(source) -> Vec<HeadingEntry>`. The caller already has a
//! tree-sitter `Tree` (from [`crate::MarkdownParser`]); this module walks the
//! existing [`crate::BlockSpan`] vector so we don't re-parse.

use std::ops::Range;

use ropey::Rope;

use crate::{BlockKind, BlockSpan};

/// One heading row for the goto-heading picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    /// 1..=6.
    pub level: u8,
    /// Heading text with ATX hashes / setext underline stripped, trimmed.
    pub text: String,
    /// 0-indexed line number of the heading's first character.
    pub line: u32,
    /// Inclusive byte start of the heading block.
    pub start_byte: usize,
}

/// Source backing for [`headings`].
///
/// Implemented for both `str` and [`Rope`] so UI paint can extract small
/// heading slices without first materializing the whole document.
pub trait HeadingSource {
    /// Total source length in UTF-8 bytes.
    fn len_bytes(&self) -> usize;
    /// Return the UTF-8 text inside `range`, or an empty string when
    /// stale decoration offsets no longer describe valid source bytes.
    fn slice_to_string(&self, range: Range<usize>) -> String;
    /// Convert a UTF-8 byte offset to a zero-indexed line number.
    fn byte_to_line(&self, byte: usize) -> u32;
}

impl HeadingSource for str {
    fn len_bytes(&self) -> usize {
        self.len()
    }

    fn slice_to_string(&self, range: Range<usize>) -> String {
        self.get(range).unwrap_or("").to_string()
    }

    fn byte_to_line(&self, byte: usize) -> u32 {
        byte_to_line_in_str(self, byte)
    }
}

impl HeadingSource for String {
    fn len_bytes(&self) -> usize {
        self.len()
    }

    fn slice_to_string(&self, range: Range<usize>) -> String {
        self.as_str().get(range).unwrap_or("").to_string()
    }

    fn byte_to_line(&self, byte: usize) -> u32 {
        byte_to_line_in_str(self.as_str(), byte)
    }
}

impl HeadingSource for Rope {
    fn len_bytes(&self) -> usize {
        self.len_bytes()
    }

    fn slice_to_string(&self, range: Range<usize>) -> String {
        if range.start > range.end || range.end > self.len_bytes() {
            return String::new();
        }
        if self.try_byte_to_char(range.start).is_err() || self.try_byte_to_char(range.end).is_err()
        {
            return String::new();
        }
        self.byte_slice(range).to_string()
    }

    fn byte_to_line(&self, byte: usize) -> u32 {
        let mut byte = byte.min(self.len_bytes());
        let char_idx = loop {
            match self.try_byte_to_char(byte) {
                Ok(char_idx) => break char_idx,
                Err(_) if byte > 0 => byte -= 1,
                Err(_) => return 0,
            }
        };
        self.char_to_line(char_idx) as u32
    }
}

fn byte_to_line_in_str(source: &str, byte: usize) -> u32 {
    let upto = byte.min(source.len());
    source.as_bytes()[..upto]
        .iter()
        .filter(|b| **b == b'\n')
        .count() as u32
}

/// Walk `spans` and produce one [`HeadingEntry`] per ATX or setext heading.
///
/// `source` is the full buffer text or rope; `start_byte` indices into
/// `spans` must be valid byte offsets.
#[must_use]
pub fn headings<S: HeadingSource + ?Sized>(spans: &[BlockSpan], source: &S) -> Vec<HeadingEntry> {
    let mut out = Vec::new();
    for span in spans {
        let level = match span.kind {
            BlockKind::Heading { level } => level,
            BlockKind::SetextHeading { level } => level,
            _ => continue,
        };
        let end_byte = span.end_byte.min(source.len_bytes());
        let raw = source.slice_to_string(span.start_byte..end_byte);
        let text = clean_heading_text(&raw, span.kind);
        let line = source.byte_to_line(span.start_byte);
        out.push(HeadingEntry {
            level,
            text,
            line,
            start_byte: span.start_byte,
        });
    }
    out
}

fn clean_heading_text(raw: &str, kind: BlockKind) -> String {
    match kind {
        BlockKind::Heading { .. } => {
            let first = raw.lines().next().unwrap_or("");
            let stripped = first.trim_start_matches('#').trim();
            stripped
                .trim_end_matches(|c: char| c == '#' || c.is_whitespace())
                .to_string()
        }
        BlockKind::SetextHeading { .. } => raw.lines().next().unwrap_or("").trim().to_string(),
        _ => raw.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{spans::block_spans, MarkdownParser};
    use ropey::Rope;

    fn extract(src: &str) -> Vec<HeadingEntry> {
        let mut p = MarkdownParser::new().unwrap();
        let tree = p.parse(src, None).unwrap();
        let spans = block_spans(&tree);
        headings(&spans, src)
    }

    #[test]
    fn extracts_atx_levels_and_text() {
        let src = "# One\n\n## Two\n\n### Three trailing #\n";
        let h = extract(src);
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].level, 1);
        assert_eq!(h[0].text, "One");
        assert_eq!(h[1].level, 2);
        assert_eq!(h[1].text, "Two");
        assert_eq!(h[2].level, 3);
        assert_eq!(h[2].text, "Three trailing");
    }

    #[test]
    fn line_numbers_zero_indexed() {
        let src = "intro\n\n# heading\n\nmore\n\n## sub\n";
        let h = extract(src);
        assert_eq!(h[0].line, 2);
        assert_eq!(h[1].line, 6);
    }

    #[test]
    fn ignores_paragraphs_and_code() {
        let src = "para\n\n```\n# fake heading inside fence\n```\n\n# real\n";
        let h = extract(src);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].text, "real");
    }

    #[test]
    fn empty_source_has_no_headings() {
        assert!(extract("").is_empty());
    }

    #[test]
    fn rope_source_matches_string_source() {
        let src = "# One\n\n## Two\n\nSetext\n------\n";
        let mut p = MarkdownParser::new().unwrap();
        let tree = p.parse(src, None).unwrap();
        let spans = block_spans(&tree);
        let rope = Rope::from_str(src);

        assert_eq!(headings(&spans, src), headings(&spans, &rope));
    }
}
