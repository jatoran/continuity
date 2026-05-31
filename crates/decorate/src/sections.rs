//! Section queries over a parsed markdown buffer.
//!
//! A *section* is the half-open byte range starting at a heading line and
//! ending at the next heading whose level is **less than or equal to** the
//! starting heading's level (or end-of-file). This matches the user-facing
//! intuition: a `## Foo` section contains every line through the next `#` or
//! `##` heading, including nested `### Bar` blocks.
//!
//! Builds on the existing [`crate::headings`] driver — no extra parse.
//!
//! Consumers:
//! - Sticky-heading breadcrumb (renders the chain of enclosing headings).
//! - Outline sidebar (renders the heading tree).
//! - Outline manipulation (`Tab` / `Shift+Tab` promote-demote, `Alt+Up/Down`
//!   move section by acting on the section bounds returned here).
//! - Slash-command context detection (which section the caret is in).
//!
//! Single-writer convention: this module is pure functions over a slice of
//! [`HeadingEntry`]. No shared state, no threading concerns.

use crate::headings::HeadingEntry;

/// Inclusive-start, exclusive-end byte range describing a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionBounds {
    /// Byte offset of the section's first character (start of the heading
    /// line).
    pub start_byte: usize,
    /// Byte offset one past the section's last character. Equals
    /// `source.len()` when the section runs to EOF.
    pub end_byte: usize,
}

/// Return the index in `headings` of the heading enclosing `byte`, or `None`
/// when `byte` is positioned before the first heading.
///
/// Enclosing = the most recent heading whose `start_byte <= byte`. The
/// section is considered to extend until the next heading of equal or higher
/// rank (lower or equal level number); after that point, a new heading
/// becomes the enclosing one.
#[must_use]
pub fn heading_index_at(headings: &[HeadingEntry], byte: usize) -> Option<usize> {
    if headings.is_empty() {
        return None;
    }
    // Binary search by start_byte: last index with start_byte <= byte.
    let mut lo = 0usize;
    let mut hi = headings.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if headings[mid].start_byte <= byte {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        None
    } else {
        Some(lo - 1)
    }
}

/// Convenience: clone of the enclosing heading at `byte`, or `None`.
#[must_use]
pub fn heading_at(headings: &[HeadingEntry], byte: usize) -> Option<HeadingEntry> {
    heading_index_at(headings, byte).map(|i| headings[i].clone())
}

/// Return the breadcrumb chain of enclosing headings for `byte`, ordered
/// outermost to innermost (H1 first).
///
/// Algorithm: walk backwards from the heading at `byte`, keeping each
/// strictly-shallower heading we encounter. The result is the unique
/// ancestor chain a tree-of-headings view would produce.
#[must_use]
pub fn heading_chain_at(headings: &[HeadingEntry], byte: usize) -> Vec<HeadingEntry> {
    let Some(idx) = heading_index_at(headings, byte) else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    let mut cur_level = headings[idx].level + 1; // any value larger than what's possible
    let mut i = idx as isize;
    while i >= 0 {
        let h = &headings[i as usize];
        if h.level < cur_level {
            chain.push(h.clone());
            cur_level = h.level;
            if cur_level == 1 {
                break;
            }
        }
        i -= 1;
    }
    chain.reverse();
    chain
}

/// Bounds of the section owned by `heading_index`. The section ends just
/// before the next heading whose level is `<=` the starting heading's level,
/// or at `source_len` (EOF) when no such heading exists.
#[must_use]
pub fn section_bounds(
    headings: &[HeadingEntry],
    heading_index: usize,
    source_len: usize,
) -> Option<SectionBounds> {
    let start = headings.get(heading_index)?;
    let mut end_byte = source_len;
    for h in headings.iter().skip(heading_index + 1) {
        if h.level <= start.level {
            end_byte = h.start_byte;
            break;
        }
    }
    Some(SectionBounds {
        start_byte: start.start_byte,
        end_byte,
    })
}

/// Convenience: section bounds for the heading enclosing `byte`.
#[must_use]
pub fn section_at(
    headings: &[HeadingEntry],
    byte: usize,
    source_len: usize,
) -> Option<SectionBounds> {
    let idx = heading_index_at(headings, byte)?;
    section_bounds(headings, idx, source_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{spans::block_spans, MarkdownParser};

    fn parse(src: &str) -> Vec<HeadingEntry> {
        let mut p = MarkdownParser::new().unwrap();
        let tree = p.parse(src, None).unwrap();
        let spans = block_spans(&tree);
        crate::headings::headings(&spans, src)
    }

    #[test]
    fn empty_source_returns_none() {
        let h = parse("");
        assert!(heading_index_at(&h, 0).is_none());
        assert!(heading_at(&h, 0).is_none());
        assert!(heading_chain_at(&h, 0).is_empty());
        assert!(section_at(&h, 0, 0).is_none());
    }

    #[test]
    fn byte_before_first_heading_has_no_section() {
        let src = "intro paragraph\n\n# First\nbody\n";
        let h = parse(src);
        assert!(heading_at(&h, 0).is_none());
        assert!(heading_at(&h, 10).is_none());
        let first_byte = h[0].start_byte;
        assert!(heading_at(&h, first_byte).is_some());
    }

    #[test]
    fn heading_at_returns_most_recent() {
        let src = "# A\n\n## B\n\n### C\n\nbody under C\n";
        let h = parse(src);
        // Caret in "body under C" → most recent heading is C.
        let body_byte = src.find("body under C").unwrap();
        let got = heading_at(&h, body_byte).unwrap();
        assert_eq!(got.text, "C");
        assert_eq!(got.level, 3);
    }

    #[test]
    fn chain_returns_strict_ancestors_outermost_first() {
        let src = "# A\n\n## B\n\n### C\n\nbody under C\n";
        let h = parse(src);
        let body_byte = src.find("body under C").unwrap();
        let chain = heading_chain_at(&h, body_byte);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].text, "A");
        assert_eq!(chain[1].text, "B");
        assert_eq!(chain[2].text, "C");
    }

    #[test]
    fn chain_skips_intermediate_siblings() {
        // Caret under D (h3 sibling of C). Chain = A → D (not A → B → C → D).
        let src = "# A\n\n## B\n\n### C\n\n## D\n\nunder D\n";
        let h = parse(src);
        let body_byte = src.find("under D").unwrap();
        let chain = heading_chain_at(&h, body_byte);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].text, "A");
        assert_eq!(chain[1].text, "D");
    }

    #[test]
    fn chain_short_circuits_at_h1() {
        let src = "# Root\n\nbody\n";
        let h = parse(src);
        let chain = heading_chain_at(&h, src.find("body").unwrap());
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].text, "Root");
        assert_eq!(chain[0].level, 1);
    }

    #[test]
    fn section_runs_to_next_equal_or_higher_heading() {
        // ## B owns its body + ### C, ends at ## D.
        let src = "# A\n\n## B\n\nb-body\n\n### C\n\nc-body\n\n## D\n\nd-body\n";
        let h = parse(src);
        let b_idx = h.iter().position(|x| x.text == "B").unwrap();
        let d_idx = h.iter().position(|x| x.text == "D").unwrap();
        let b_bounds = section_bounds(&h, b_idx, src.len()).unwrap();
        assert_eq!(b_bounds.start_byte, h[b_idx].start_byte);
        assert_eq!(b_bounds.end_byte, h[d_idx].start_byte);
    }

    #[test]
    fn section_runs_to_eof_when_no_terminator() {
        let src = "# Only\n\nbody only\n";
        let h = parse(src);
        let bounds = section_bounds(&h, 0, src.len()).unwrap();
        assert_eq!(bounds.start_byte, h[0].start_byte);
        assert_eq!(bounds.end_byte, src.len());
    }

    #[test]
    fn section_includes_nested_subsections() {
        let src = "# A\n\nbody-a\n\n## B\n\nbody-b\n\n### C\n\nbody-c\n";
        let h = parse(src);
        // A's section covers everything (nothing equal-or-higher follows).
        let a_bounds = section_bounds(&h, 0, src.len()).unwrap();
        assert_eq!(a_bounds.end_byte, src.len());
        // B's section runs to EOF (C is deeper, not a terminator).
        let b_idx = h.iter().position(|x| x.text == "B").unwrap();
        let b_bounds = section_bounds(&h, b_idx, src.len()).unwrap();
        assert_eq!(b_bounds.end_byte, src.len());
    }

    #[test]
    fn heading_index_at_boundary_returns_owning_heading() {
        let src = "# A\n\n## B\n\nbody\n";
        let h = parse(src);
        // Byte exactly at B's start_byte → B is the enclosing heading.
        let b_idx = h.iter().position(|x| x.text == "B").unwrap();
        let at_b = heading_index_at(&h, h[b_idx].start_byte).unwrap();
        assert_eq!(at_b, b_idx);
    }
}
