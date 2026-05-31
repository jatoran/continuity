//! §H1 — focus-mode span detection.
//!
//! Computes the source-byte range covered by the caret's *line*,
//! *sentence*, or *paragraph* — used by the focus-mode dim pass to
//! decide which source ranges stay at full contrast.
//!
//! These are pure functions over a string slice; no parse state.
//!
//! Span semantics:
//! - **Line**: bytes of the caret's source line (`\n` terminators
//!   excluded from the span; the dim pass paints up to the line end).
//! - **Sentence**: bytes back to the previous `.`, `!`, `?`, or
//!   paragraph break, forward to the next such terminator
//!   (terminator included). Plain-prose punctuation-based — markdown
//!   block boundaries also terminate.
//! - **Paragraph**: bytes back to the previous blank line, forward to
//!   the next blank line. Equivalent to a markdown paragraph block
//!   when there are no nested structures.

/// Inclusive-start, exclusive-end byte range describing a focus span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusSpan {
    /// Byte offset of the span's first character.
    pub start: usize,
    /// Byte offset one past the span's last character.
    pub end: usize,
}

impl FocusSpan {
    /// `true` when `byte` is inside the span.
    #[must_use]
    pub fn contains(&self, byte: usize) -> bool {
        byte >= self.start && byte < self.end
    }
}

/// Line span: bytes of the caret's source line. Excludes the `\n`
/// terminator so callers can paint up to the visual line end.
#[must_use]
pub fn line_span(source: &str, byte: usize) -> FocusSpan {
    let byte = byte.min(source.len());
    let bytes = source.as_bytes();
    let start = bytes[..byte]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let end = bytes[byte..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(source.len(), |i| byte + i);
    FocusSpan { start, end }
}

/// Paragraph span: bytes back to the previous blank line and forward
/// to the next blank line. Blank lines are excluded from the span.
#[must_use]
pub fn paragraph_span(source: &str, byte: usize) -> FocusSpan {
    let byte = byte.min(source.len());
    let bytes = source.as_bytes();
    let start = find_paragraph_start(bytes, byte);
    let end = find_paragraph_end(bytes, byte);
    FocusSpan { start, end }
}

fn find_paragraph_start(bytes: &[u8], byte: usize) -> usize {
    // Walk backward line-by-line; stop one past the most recent blank line.
    let mut pos = byte;
    loop {
        // Move to start of current line.
        let line_start = bytes[..pos]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        if line_start == 0 {
            return 0;
        }
        // Check whether the prior line (line_start-1 is the '\n' before
        // this line; bytes[prev_line_start..line_start-1] is its body)
        // is blank.
        let prev_line_start = bytes[..line_start - 1]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let prev_line = &bytes[prev_line_start..line_start - 1];
        if is_blank_line(prev_line) {
            return line_start;
        }
        pos = line_start - 1; // step into prior line
    }
}

fn find_paragraph_end(bytes: &[u8], byte: usize) -> usize {
    // Walk forward line-by-line; stop just before the next blank line.
    let mut pos = byte;
    loop {
        let line_end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| pos + i);
        if line_end == bytes.len() {
            return line_end;
        }
        // bytes[line_end] is '\n'. Next line starts at line_end+1.
        let next_line_start = line_end + 1;
        let next_line_end = bytes[next_line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| next_line_start + i);
        let next_line = &bytes[next_line_start..next_line_end];
        if is_blank_line(next_line) {
            return line_end;
        }
        pos = next_line_start;
    }
}

fn is_blank_line(line: &[u8]) -> bool {
    line.iter().all(|b| matches!(b, b' ' | b'\t' | b'\r'))
}

/// Sentence span: bytes back to the previous sentence terminator (or
/// paragraph break) and forward to the next sentence terminator (or
/// paragraph break). Terminators are `.`, `!`, `?` followed by a
/// whitespace boundary; the terminator byte is included in the span.
#[must_use]
pub fn sentence_span(source: &str, byte: usize) -> FocusSpan {
    let para = paragraph_span(source, byte);
    let bytes = source.as_bytes();
    let byte = byte.min(source.len());
    // Walk backward inside the paragraph for the previous terminator.
    let start = {
        let mut i = byte;
        let mut found = para.start;
        while i > para.start {
            let b = bytes[i - 1];
            if (b == b'.' || b == b'!' || b == b'?')
                && bytes
                    .get(i)
                    .is_none_or(|n| matches!(*n, b' ' | b'\t' | b'\n' | b'\r'))
            {
                // Skip terminator + following whitespace.
                let mut j = i;
                while j < para.end && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                found = j;
                break;
            }
            i -= 1;
        }
        found
    };
    // Walk forward for the next terminator.
    let end = {
        let mut i = byte;
        let mut found = para.end;
        while i < para.end {
            let b = bytes[i];
            if (b == b'.' || b == b'!' || b == b'?')
                && bytes
                    .get(i + 1)
                    .is_none_or(|n| matches!(*n, b' ' | b'\t' | b'\n' | b'\r'))
            {
                found = (i + 1).min(para.end);
                break;
            }
            i += 1;
        }
        found
    };
    FocusSpan { start, end }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_span_returns_current_line_bytes() {
        let src = "first line\nsecond line\nthird\n";
        // Caret in "second".
        let byte = src.find("second").unwrap();
        let span = line_span(src, byte);
        assert_eq!(&src[span.start..span.end], "second line");
    }

    #[test]
    fn line_span_at_eof_with_no_trailing_newline() {
        let src = "only line";
        let span = line_span(src, 3);
        assert_eq!(span.start, 0);
        assert_eq!(span.end, src.len());
    }

    #[test]
    fn line_span_at_start_of_buffer() {
        let src = "alpha\nbeta\n";
        let span = line_span(src, 0);
        assert_eq!(&src[span.start..span.end], "alpha");
    }

    #[test]
    fn paragraph_span_spans_contiguous_non_blank_lines() {
        let src = "first paragraph line\nsecond line\n\nnext paragraph\n";
        let byte = src.find("second").unwrap();
        let span = paragraph_span(src, byte);
        assert_eq!(
            &src[span.start..span.end],
            "first paragraph line\nsecond line"
        );
    }

    #[test]
    fn paragraph_span_stops_at_blank_lines() {
        let src = "para A\n\npara B\nstill B\n\npara C\n";
        let byte = src.find("still B").unwrap();
        let span = paragraph_span(src, byte);
        assert_eq!(&src[span.start..span.end], "para B\nstill B");
    }

    #[test]
    fn paragraph_span_single_paragraph_buffer() {
        let src = "single para body\nstill same para\n";
        let span = paragraph_span(src, 5);
        assert_eq!(span.start, 0);
        assert_eq!(span.end, src.len() - 1);
    }

    #[test]
    fn sentence_span_back_to_prev_terminator() {
        let src = "Hello world. This is a test. Final sentence.";
        let byte = src.find("test").unwrap();
        let span = sentence_span(src, byte);
        assert_eq!(&src[span.start..span.end], "This is a test.");
    }

    #[test]
    fn sentence_span_handles_question_marks_and_bangs() {
        let src = "Wait! What? Now we begin.";
        let byte = src.find("What").unwrap();
        let span = sentence_span(src, byte);
        assert_eq!(&src[span.start..span.end], "What?");
    }

    #[test]
    fn sentence_span_clamped_to_paragraph() {
        let src = "First sentence here.\n\nSecond para no period";
        let byte = src.find("Second").unwrap();
        let span = sentence_span(src, byte);
        // Should not bleed into prior paragraph; should run to paragraph end.
        assert!(span.start >= src.find("Second").unwrap());
        assert_eq!(span.end, src.len());
    }

    #[test]
    fn focus_span_contains() {
        let span = FocusSpan { start: 5, end: 10 };
        assert!(span.contains(5));
        assert!(span.contains(9));
        assert!(!span.contains(10));
        assert!(!span.contains(4));
    }

    #[test]
    fn empty_source_yields_empty_spans() {
        let src = "";
        assert_eq!(line_span(src, 0), FocusSpan { start: 0, end: 0 });
        assert_eq!(paragraph_span(src, 0), FocusSpan { start: 0, end: 0 });
        assert_eq!(sentence_span(src, 0), FocusSpan { start: 0, end: 0 });
    }
}
