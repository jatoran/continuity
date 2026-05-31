//! Phase F3 — inline color / highlight markup parser.
//!
//! Two continuity-only extensions to plain markdown:
//!
//! - `==text==` — default highlight (color picked by the theme via the
//!   `editor.inline_highlight.*` keys).
//! - `{#rrggbb:text}` or `{#rgb:text}` (also `#rrggbbaa` / `#rgba`) —
//!   custom hex foreground color applied to `text`.
//!
//! Source bytes stay plain markdown — these extensions are *recognised*
//! at decoration time (much like autolink) so the saved file remains
//! interoperable. This module is the pure parser that turns a source
//! string into a list of [`InlineColorSpan`]s; the renderer consumes
//! that list and paints the runs.
//!
//! Thread ownership: pure, callable from any thread.

use std::ops::Range;

/// One painted inline-color run discovered in a source byte string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineColorSpan {
    /// Byte range covering the entire markup (`==…==` or `{#…:…}`),
    /// inclusive of the delimiters.
    pub outer: Range<usize>,
    /// Byte range covering the user-visible text (the content between
    /// delimiters). The renderer paints this run with the resolved
    /// color; the delimiter bytes are hidden via the display map.
    pub inner: Range<usize>,
    /// Kind tag — `Highlight` for `==…==`; `Hex` for `{#…:…}` with
    /// the packed RGBA value (alpha = 0xFF when only 3 / 6 hex digits
    /// were supplied).
    pub kind: InlineColorKind,
}

/// Discriminates the two F3 markup flavours.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlineColorKind {
    /// `==text==` — uses the theme's highlight color.
    Highlight,
    /// `{#hex:text}` — uses the explicit hex value. Stored as packed
    /// `0xRRGGBBAA`.
    Hex(u32),
}

/// Walk `source` and produce every inline-color span. Spans are
/// returned in document order; nested or overlapping spans are not
/// emitted — the outermost delimited region wins.
#[must_use]
pub fn inline_color_spans(source: &str) -> Vec<InlineColorSpan> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'=' && bytes[i + 1] == b'=' {
            if let Some(span) = parse_highlight(source, i) {
                let end = span.outer.end;
                out.push(span);
                i = end;
                continue;
            }
        }
        if bytes[i] == b'{' {
            if let Some(span) = parse_hex(source, i) {
                let end = span.outer.end;
                out.push(span);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn parse_highlight(source: &str, start: usize) -> Option<InlineColorSpan> {
    debug_assert!(source.is_char_boundary(start));
    let bytes = source.as_bytes();
    if start + 2 > bytes.len() || bytes[start] != b'=' || bytes[start + 1] != b'=' {
        return None;
    }
    let inner_start = start + 2;
    // Empty highlight (`====`) is not a markup span.
    let mut i = inner_start;
    while i + 1 < bytes.len() {
        if bytes[i] == b'=' && bytes[i + 1] == b'=' && i > inner_start {
            let inner_end = i;
            let outer_end = i + 2;
            // Cap a markup span to a single line — `==…==` is inline.
            if source[inner_start..inner_end].contains('\n') {
                return None;
            }
            return Some(InlineColorSpan {
                outer: start..outer_end,
                inner: inner_start..inner_end,
                kind: InlineColorKind::Highlight,
            });
        }
        i += 1;
    }
    None
}

fn parse_hex(source: &str, start: usize) -> Option<InlineColorSpan> {
    let bytes = source.as_bytes();
    if bytes.get(start)? != &b'{' || bytes.get(start + 1)? != &b'#' {
        return None;
    }
    // Scan hex digits after `{#`.
    let hex_start = start + 2;
    let mut hex_end = hex_start;
    while hex_end < bytes.len() && bytes[hex_end].is_ascii_hexdigit() {
        hex_end += 1;
    }
    let hex_len = hex_end - hex_start;
    if !matches!(hex_len, 3 | 4 | 6 | 8) {
        return None;
    }
    if bytes.get(hex_end)? != &b':' {
        return None;
    }
    let inner_start = hex_end + 1;
    // Find the matching `}` on the same line.
    let mut i = inner_start;
    while i < bytes.len() {
        match bytes[i] {
            b'}' => {
                if i == inner_start {
                    return None;
                }
                let rgba = parse_hex_rgba(&source[hex_start..hex_end])?;
                return Some(InlineColorSpan {
                    outer: start..i + 1,
                    inner: inner_start..i,
                    kind: InlineColorKind::Hex(rgba),
                });
            }
            b'\n' => return None,
            _ => i += 1,
        }
    }
    None
}

fn hex_digit(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'a'..=b'f' => (c - b'a') as u32 + 10,
        b'A'..=b'F' => (c - b'A') as u32 + 10,
        _ => 0,
    }
}

/// Parse a hex string (`rgb` / `rgba` / `rrggbb` / `rrggbbaa`) into a
/// packed `0xRRGGBBAA` u32. Returns `None` for any other length.
#[must_use]
pub fn parse_hex_rgba(hex: &str) -> Option<u32> {
    let bytes = hex.as_bytes();
    let (r, g, b, a) = match bytes.len() {
        3 => (
            hex_digit(bytes[0]) * 0x11,
            hex_digit(bytes[1]) * 0x11,
            hex_digit(bytes[2]) * 0x11,
            0xFF,
        ),
        4 => (
            hex_digit(bytes[0]) * 0x11,
            hex_digit(bytes[1]) * 0x11,
            hex_digit(bytes[2]) * 0x11,
            hex_digit(bytes[3]) * 0x11,
        ),
        6 => (
            hex_digit(bytes[0]) * 16 + hex_digit(bytes[1]),
            hex_digit(bytes[2]) * 16 + hex_digit(bytes[3]),
            hex_digit(bytes[4]) * 16 + hex_digit(bytes[5]),
            0xFF,
        ),
        8 => (
            hex_digit(bytes[0]) * 16 + hex_digit(bytes[1]),
            hex_digit(bytes[2]) * 16 + hex_digit(bytes[3]),
            hex_digit(bytes[4]) * 16 + hex_digit(bytes[5]),
            hex_digit(bytes[6]) * 16 + hex_digit(bytes[7]),
        ),
        _ => return None,
    };
    Some((r << 24) | (g << 16) | (b << 8) | a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_is_recognised() {
        let s = "intro ==yellow== outro";
        let spans = inline_color_spans(s);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, InlineColorKind::Highlight);
        assert_eq!(&s[spans[0].inner.clone()], "yellow");
        assert_eq!(&s[spans[0].outer.clone()], "==yellow==");
    }

    #[test]
    fn highlight_must_span_one_line() {
        let s = "==first\nsecond==";
        assert!(inline_color_spans(s).is_empty());
    }

    #[test]
    fn empty_highlight_is_not_a_span() {
        assert!(inline_color_spans("====").is_empty());
    }

    #[test]
    fn hex_3_digit_form_expands_to_full_byte() {
        let s = "{#f06:pink}";
        let spans = inline_color_spans(s);
        assert_eq!(spans.len(), 1);
        match spans[0].kind {
            InlineColorKind::Hex(rgba) => {
                // 0xff 0x00 0x66 0xff packed.
                assert_eq!(rgba, 0xFF0066FF);
            }
            _ => panic!("expected Hex variant"),
        }
        assert_eq!(&s[spans[0].inner.clone()], "pink");
    }

    #[test]
    fn hex_6_digit_form_recognised() {
        let s = "{#abcdef:foo}";
        let spans = inline_color_spans(s);
        assert_eq!(spans.len(), 1);
        match spans[0].kind {
            InlineColorKind::Hex(rgba) => assert_eq!(rgba, 0xABCDEFFF),
            _ => panic!("expected Hex"),
        }
    }

    #[test]
    fn hex_with_alpha_keeps_alpha_byte() {
        let s = "{#11223344:a}";
        let spans = inline_color_spans(s);
        assert_eq!(spans.len(), 1);
        match spans[0].kind {
            InlineColorKind::Hex(rgba) => assert_eq!(rgba, 0x11223344),
            _ => panic!("expected Hex"),
        }
    }

    #[test]
    fn hex_with_invalid_digit_is_rejected() {
        let s = "{#xyz:foo}";
        assert!(inline_color_spans(s).is_empty());
    }

    #[test]
    fn hex_without_colon_is_rejected() {
        let s = "{#abc abc}";
        assert!(inline_color_spans(s).is_empty());
    }

    #[test]
    fn hex_must_close_on_same_line() {
        let s = "{#abc:foo\nbar}";
        assert!(inline_color_spans(s).is_empty());
    }

    #[test]
    fn multiple_spans_in_order() {
        let s = "==a== and {#fff:b}";
        let spans = inline_color_spans(s);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].kind, InlineColorKind::Highlight);
        assert!(matches!(spans[1].kind, InlineColorKind::Hex(_)));
        // First span ends before second begins.
        assert!(spans[0].outer.end <= spans[1].outer.start);
    }

    #[test]
    fn adjacent_spans_do_not_consume_each_other() {
        let s = "=={#abc:x}== rest";
        // The outer `==…==` wins; the `{#…:…}` is just bytes inside.
        let spans = inline_color_spans(s);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, InlineColorKind::Highlight);
    }

    #[test]
    fn parse_hex_rgba_handles_each_length() {
        assert_eq!(parse_hex_rgba("abc"), Some(0xAABBCCFF));
        assert_eq!(parse_hex_rgba("abcd"), Some(0xAABBCCDD));
        assert_eq!(parse_hex_rgba("aabbcc"), Some(0xAABBCCFF));
        assert_eq!(parse_hex_rgba("aabbccdd"), Some(0xAABBCCDD));
        assert!(parse_hex_rgba("ab").is_none());
        assert!(parse_hex_rgba("abcde").is_none());
    }
}
