//! Whole-document footnote definition scanning.
//!
//! Inline references are found by the block-local text scanner; definitions
//! need a document pass so their bodies can be resolved for hover peeks and
//! reverse navigation.

use crate::inline::{ByteRange, InlineKind, InlineSpan};

/// Return one [`InlineKind::FootnoteDefinition`] span per `[^label]: body`
/// definition in `source`.
#[must_use]
pub fn footnote_definition_spans(source: &str) -> Vec<InlineSpan> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut line_start = 0usize;
    while line_start < bytes.len() {
        let (line_end, next_line) = line_bounds(bytes, line_start);
        let Some(parsed) = parse_definition_line(bytes, line_start, line_end) else {
            line_start = next_line;
            continue;
        };
        let (body_end, skip_to) = definition_body_extent(bytes, line_end, next_line);
        out.push(InlineSpan {
            kind: InlineKind::FootnoteDefinition {
                label: parsed.label.to_string(),
                body_range: ByteRange::new(parsed.body_start, body_end),
            },
            range: ByteRange::new(parsed.label_start, parsed.label_end),
        });
        line_start = skip_to;
    }
    out
}

struct ParsedDefinition<'a> {
    label: &'a str,
    label_start: usize,
    label_end: usize,
    body_start: usize,
}

fn parse_definition_line<'a>(
    bytes: &'a [u8],
    line_start: usize,
    line_end: usize,
) -> Option<ParsedDefinition<'a>> {
    let mut i = line_start;
    let mut spaces = 0usize;
    while i < line_end && bytes[i] == b' ' && spaces < 3 {
        i += 1;
        spaces += 1;
    }
    if i + 3 >= line_end || bytes[i] != b'[' || bytes[i + 1] != b'^' {
        return None;
    }
    let label_start = i;
    let label_text_start = i + 2;
    i = label_text_start;
    while i < line_end && bytes[i] != b']' {
        if !is_label_byte(bytes[i]) {
            return None;
        }
        i += 1;
    }
    if i == label_text_start || i + 1 >= line_end || bytes[i] != b']' || bytes[i + 1] != b':' {
        return None;
    }
    let label = std::str::from_utf8(&bytes[label_text_start..i]).ok()?;
    let mut body_start = i + 2;
    if body_start < line_end && matches!(bytes[body_start], b' ' | b'\t') {
        body_start += 1;
    }
    Some(ParsedDefinition {
        label,
        label_start,
        label_end: i + 1,
        body_start,
    })
}

fn definition_body_extent(bytes: &[u8], first_line_end: usize, next_line: usize) -> (usize, usize) {
    let mut body_end = first_line_end;
    let mut cursor = next_line;
    while cursor < bytes.len() {
        let (line_end, next) = line_bounds(bytes, cursor);
        if !is_continuation_line(bytes, cursor, line_end) {
            break;
        }
        body_end = line_end;
        cursor = next;
    }
    (body_end, cursor)
}

fn is_continuation_line(bytes: &[u8], line_start: usize, line_end: usize) -> bool {
    if line_start >= line_end {
        return false;
    }
    if bytes[line_start] == b'\t' {
        return true;
    }
    let mut spaces = 0usize;
    let mut i = line_start;
    while i < line_end && bytes[i] == b' ' {
        spaces += 1;
        i += 1;
    }
    spaces >= 4
}

fn line_bounds(bytes: &[u8], line_start: usize) -> (usize, usize) {
    let mut line_end = line_start;
    while line_end < bytes.len() && bytes[line_end] != b'\n' {
        line_end += 1;
    }
    let mut content_end = line_end;
    if content_end > line_start && bytes[content_end - 1] == b'\r' {
        content_end -= 1;
    }
    let next_line = if line_end < bytes.len() {
        line_end + 1
    } else {
        line_end
    };
    (content_end, next_line)
}

fn is_label_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_definition_body() {
        let source = "a [^1]\n\n[^1]: first line\n    second line\nnext\n";
        let spans = footnote_definition_spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].range, ByteRange::new(8, 12));
        match &spans[0].kind {
            InlineKind::FootnoteDefinition { label, body_range } => {
                assert_eq!(label, "1");
                assert_eq!(
                    &source[body_range.start..body_range.end],
                    "first line\n    second line"
                );
            }
            other => panic!("unexpected kind: {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_labels() {
        let source = "[^bad label]: body\n";
        assert!(footnote_definition_spans(source).is_empty());
    }
}
