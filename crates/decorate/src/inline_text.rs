//! Hand-rolled inline scanner for text-bearing markdown blocks.
//!
//! Extracted from `inline.rs` to keep both files under the 600-line cap.
//! Pure logic; no globals, no I/O. Spans returned have document-absolute
//! byte ranges (offsets are relative to `base` which the caller supplies).

use crate::inline::{ByteRange, InlineKind, InlineSpan, MarkerKind};

/// Scan emphasis/strong/strike/code spans, links, and image refs in a
/// text-bearing block. Conservative — ambiguous nesting falls back to
/// "no inline match" rather than producing wrong byte ranges.
pub(crate) fn scan_text_inlines(base: usize, src: &str, out: &mut Vec<InlineSpan>) {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'`' => {
                let open_start = i;
                let mut open_count = 0usize;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                    open_count += 1;
                }
                let body_start = i;
                let mut found_end = None;
                while i < bytes.len() {
                    if bytes[i] == b'`' {
                        let run_start = i;
                        let mut run_count = 0usize;
                        while i < bytes.len() && bytes[i] == b'`' {
                            i += 1;
                            run_count += 1;
                        }
                        if run_count == open_count {
                            found_end = Some((run_start, i));
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                if let Some((close_start, close_end)) = found_end {
                    out.push(InlineSpan {
                        kind: InlineKind::Marker(MarkerKind::CodeDelim),
                        range: ByteRange::new(base + open_start, base + body_start),
                    });
                    out.push(InlineSpan {
                        kind: InlineKind::Code,
                        range: ByteRange::new(base + body_start, base + close_start),
                    });
                    out.push(InlineSpan {
                        kind: InlineKind::Marker(MarkerKind::CodeDelim),
                        range: ByteRange::new(base + close_start, base + close_end),
                    });
                }
            }
            b'!' if i + 1 < bytes.len() && bytes[i + 1] == b'[' => {
                if let Some((alt, url, end)) = parse_link_or_image(bytes, i + 1) {
                    out.push(InlineSpan {
                        kind: InlineKind::ImageRef {
                            alt_range: ByteRange::new(base + alt.0, base + alt.1),
                            url_range: ByteRange::new(base + url.0, base + url.1),
                        },
                        range: ByteRange::new(base + i, base + end),
                    });
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'[' if i + 1 < bytes.len() && bytes[i + 1] == b'^' => {
                if let Some((label, end)) = parse_footnote_reference(bytes, i) {
                    out.push(InlineSpan {
                        kind: InlineKind::FootnoteReference {
                            label: label.to_string(),
                        },
                        range: ByteRange::new(base + i, base + end),
                    });
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'[' => {
                if let Some((text, url, end)) = parse_link_or_image(bytes, i) {
                    out.push(InlineSpan {
                        kind: InlineKind::Link {
                            text_range: ByteRange::new(base + text.0, base + text.1),
                            url_range: ByteRange::new(base + url.0, base + url.1),
                        },
                        range: ByteRange::new(base + i, base + end),
                    });
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'*' | b'_' => {
                if let Some((kind, mark_kind, body_start, body_end, end)) =
                    parse_emphasis_run(bytes, i)
                {
                    let open_end = body_start;
                    let close_start = body_end;
                    out.push(InlineSpan {
                        kind: InlineKind::Marker(mark_kind),
                        range: ByteRange::new(base + i, base + open_end),
                    });
                    out.push(InlineSpan {
                        kind,
                        range: ByteRange::new(base + body_start, base + body_end),
                    });
                    out.push(InlineSpan {
                        kind: InlineKind::Marker(mark_kind),
                        range: ByteRange::new(base + close_start, base + end),
                    });
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'~' if i + 1 < bytes.len() && bytes[i + 1] == b'~' => {
                let open_start = i;
                let body_start = i + 2;
                let mut j = body_start;
                let mut found = None;
                while j + 1 < bytes.len() {
                    if bytes[j] == b'~' && bytes[j + 1] == b'~' {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(close_start) = found {
                    let close_end = close_start + 2;
                    out.push(InlineSpan {
                        kind: InlineKind::Marker(MarkerKind::StrikeDelim),
                        range: ByteRange::new(base + open_start, base + body_start),
                    });
                    out.push(InlineSpan {
                        kind: InlineKind::Strikethrough,
                        range: ByteRange::new(base + body_start, base + close_start),
                    });
                    out.push(InlineSpan {
                        kind: InlineKind::Marker(MarkerKind::StrikeDelim),
                        range: ByteRange::new(base + close_start, base + close_end),
                    });
                    i = close_end;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
}

fn parse_footnote_reference(bytes: &[u8], start: usize) -> Option<(&str, usize)> {
    if start + 3 >= bytes.len() || bytes[start] != b'[' || bytes[start + 1] != b'^' {
        return None;
    }
    let label_start = start + 2;
    let mut i = label_start;
    while i < bytes.len() && bytes[i] != b']' && bytes[i] != b'\n' {
        if !is_footnote_label_byte(bytes[i]) {
            return None;
        }
        i += 1;
    }
    if i == label_start || i >= bytes.len() || bytes[i] != b']' {
        return None;
    }
    let end = i + 1;
    // A definition label (`[^x]:`) is handled by the whole-document
    // definition scanner, not by the reference path.
    if end < bytes.len() && bytes[end] == b':' {
        return None;
    }
    let label = std::str::from_utf8(&bytes[label_start..i]).ok()?;
    Some((label, end))
}

fn is_footnote_label_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

/// Try to parse `[text](url)` starting at `lbrack` (the `[`).
#[allow(clippy::type_complexity)]
fn parse_link_or_image(
    bytes: &[u8],
    lbrack: usize,
) -> Option<((usize, usize), (usize, usize), usize)> {
    if lbrack >= bytes.len() || bytes[lbrack] != b'[' {
        return None;
    }
    let text_start = lbrack + 1;
    let mut depth = 1i32;
    let mut i = text_start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b'\n' => return None,
            _ => {}
        }
        if depth == 0 {
            break;
        }
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b']' {
        return None;
    }
    let text_end = i;
    if i + 1 >= bytes.len() || bytes[i + 1] != b'(' {
        return None;
    }
    let url_start = i + 2;
    i = url_start;
    while i < bytes.len() && bytes[i] != b')' && bytes[i] != b'\n' {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b')' {
        return None;
    }
    let url_end = i;
    let end = i + 1;
    Some(((text_start, text_end), (url_start, url_end), end))
}

/// Parse an emphasis run starting at `start` (`*` or `_`).
#[allow(clippy::type_complexity)]
fn parse_emphasis_run(
    bytes: &[u8],
    start: usize,
) -> Option<(InlineKind, MarkerKind, usize, usize, usize)> {
    if start >= bytes.len() {
        return None;
    }
    let ch = bytes[start];
    if ch != b'*' && ch != b'_' {
        return None;
    }
    let mut open_count = 0usize;
    let mut i = start;
    while i < bytes.len() && bytes[i] == ch && open_count < 3 {
        i += 1;
        open_count += 1;
    }
    if open_count == 0 {
        return None;
    }
    if i >= bytes.len() || bytes[i] == b' ' || bytes[i] == b'\n' || bytes[i] == b'\t' {
        return None;
    }
    let body_start = i;
    let mut close_start = None;
    let mut j = body_start;
    while j < bytes.len() {
        if bytes[j] == b'\n' {
            return None;
        }
        if bytes[j] == ch && j > body_start && bytes[j - 1] != b' ' && bytes[j - 1] != b'\t' {
            let mut run = 0usize;
            let run_start = j;
            while j < bytes.len() && bytes[j] == ch && run < open_count {
                j += 1;
                run += 1;
            }
            if run >= open_count {
                close_start = Some((run_start, j));
                break;
            }
            continue;
        }
        j += 1;
    }
    let (close_pos, end) = close_start?;
    let kind = match open_count {
        1 => InlineKind::Emphasis,
        2 => InlineKind::Strong,
        _ => InlineKind::Strong,
    };
    Some((kind, MarkerKind::EmphasisDelim, body_start, close_pos, end))
}
