//! Phase F2 — Markdown table-of-contents formatting.
//!
//! Pure functions over a heading list (from [`crate::headings::headings`])
//! that produce the markdown text the `markdown.insert_toc` command writes
//! into the rope. The TOC is bounded by GFM-style HTML comment markers
//! (`<!-- toc -->` / `<!-- /toc -->`) so [`markdown.refresh_toc`] can find
//! the previously-generated block and replace it.
//!
//! Anchor slugs follow the GFM rules used by GitHub's content renderer:
//!
//! - Lowercase every ASCII letter.
//! - Drop every character that is not alphanumeric, ASCII space, or `-`.
//! - Replace runs of whitespace with a single `-`.
//! - Suffix duplicate slugs with `-1`, `-2`, … in encounter order so two
//!   `## Setup` headings produce `setup` and `setup-1`.
//!
//! Thread ownership: stateless, callable from any thread.

use crate::headings::HeadingEntry;

/// HTML-comment marker that opens a generated TOC block.
pub const TOC_OPEN_MARKER: &str = "<!-- toc -->";

/// HTML-comment marker that closes a generated TOC block.
pub const TOC_CLOSE_MARKER: &str = "<!-- /toc -->";

/// GFM-compatible heading slug — see module docs for the rules.
#[must_use]
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_dash = false;
    for c in text.chars() {
        let push = if c.is_ascii_alphanumeric() {
            prev_dash = false;
            Some(c.to_ascii_lowercase())
        } else if c == '-' || c == '_' {
            prev_dash = false;
            Some(c)
        } else if c.is_whitespace() {
            if prev_dash {
                None
            } else {
                prev_dash = true;
                Some('-')
            }
        } else {
            None
        };
        if let Some(ch) = push {
            out.push(ch);
        }
    }
    // Trim leading/trailing `-` so `## Foo  ` doesn't slug to `foo-`.
    out.trim_matches('-').to_string()
}

/// Build the markdown TOC block for `headings`, wrapped in the marker
/// comment pair so [`refresh_in_place`] can find it later.
///
/// `min_level` and `max_level` clamp which headings are listed
/// (inclusive). Defaults are `1..=6` via [`format_toc`].
///
/// Output shape (example with `## Foo` and `### Bar`):
///
/// ```text
/// <!-- toc -->
/// - [Foo](#foo)
///   - [Bar](#bar)
/// <!-- /toc -->
/// ```
///
/// Indentation is two spaces per relative level past the shallowest
/// heading; the shallowest heading sits at column 0 so a buffer that
/// only has `## …` headings doesn't get spuriously indented.
#[must_use]
pub fn format_toc_with_levels(headings: &[HeadingEntry], min_level: u8, max_level: u8) -> String {
    let filtered: Vec<&HeadingEntry> = headings
        .iter()
        .filter(|h| h.level >= min_level && h.level <= max_level)
        .collect();
    if filtered.is_empty() {
        return format!("{TOC_OPEN_MARKER}\n{TOC_CLOSE_MARKER}\n");
    }
    let base_level = filtered.iter().map(|h| h.level).min().unwrap_or(1);
    let slugs = unique_slugs(&filtered);
    let mut out = String::new();
    out.push_str(TOC_OPEN_MARKER);
    out.push('\n');
    for (entry, slug) in filtered.iter().zip(slugs.iter()) {
        let indent_steps = (entry.level - base_level) as usize;
        for _ in 0..indent_steps {
            out.push_str("  ");
        }
        out.push_str("- [");
        out.push_str(&entry.text);
        out.push_str("](#");
        out.push_str(slug);
        out.push_str(")\n");
    }
    out.push_str(TOC_CLOSE_MARKER);
    out.push('\n');
    out
}

/// Default-level convenience wrapper for [`format_toc_with_levels`].
#[must_use]
pub fn format_toc(headings: &[HeadingEntry]) -> String {
    format_toc_with_levels(headings, 1, 6)
}

/// Locate the byte range of an existing TOC block delimited by
/// [`TOC_OPEN_MARKER`] / [`TOC_CLOSE_MARKER`] in `source`. Returns the
/// half-open `[start, end)` byte range covering the marker lines plus
/// every line between them — the trailing `\n` after `</toc>` is *not*
/// included, so the caller can replace this exact range with the output
/// of [`format_toc`] (which already includes a final newline).
///
/// Returns `None` if either marker is missing or the close marker
/// precedes the open marker.
#[must_use]
pub fn find_toc_block(source: &str) -> Option<(usize, usize)> {
    let open = source.find(TOC_OPEN_MARKER)?;
    let after_open = open + TOC_OPEN_MARKER.len();
    let rel_close = source[after_open..].find(TOC_CLOSE_MARKER)?;
    let close = after_open + rel_close + TOC_CLOSE_MARKER.len();
    Some((open, close))
}

/// Rope-backed counterpart of [`find_toc_block`].
///
/// Scans rope chunks directly and keeps only a marker-sized overlap
/// buffer, so callers do not have to materialize the whole document to
/// refresh a generated TOC.
#[must_use]
pub fn find_toc_block_in_rope(source: &ropey::Rope) -> Option<(usize, usize)> {
    let open = find_bytes_in_rope(source, TOC_OPEN_MARKER.as_bytes(), 0)?;
    let after_open = open + TOC_OPEN_MARKER.len();
    let close = find_bytes_in_rope(source, TOC_CLOSE_MARKER.as_bytes(), after_open)?
        + TOC_CLOSE_MARKER.len();
    Some((open, close))
}

fn find_bytes_in_rope(source: &ropey::Rope, needle: &[u8], start_byte: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start_byte.min(source.len_bytes()));
    }
    let carry_limit = needle.len().saturating_sub(1);
    let mut byte_offset = 0usize;
    let mut carry: Vec<u8> = Vec::new();
    for chunk in source.chunks() {
        let chunk_start = byte_offset;
        let bytes = chunk.as_bytes();
        byte_offset = byte_offset.saturating_add(bytes.len());
        if byte_offset <= start_byte {
            continue;
        }
        let start_in_chunk = start_byte.saturating_sub(chunk_start).min(bytes.len());
        let scan = &bytes[start_in_chunk..];
        let combined_start = chunk_start
            .saturating_add(start_in_chunk)
            .saturating_sub(carry.len());
        let mut combined = Vec::with_capacity(carry.len() + scan.len());
        combined.extend_from_slice(&carry);
        combined.extend_from_slice(scan);
        if let Some(idx) = combined
            .windows(needle.len())
            .position(|window| window == needle)
        {
            return Some(combined_start + idx);
        }
        carry.clear();
        let carry_len = carry_limit.min(combined.len());
        carry.extend_from_slice(&combined[combined.len() - carry_len..]);
    }
    None
}

fn unique_slugs(entries: &[&HeadingEntry]) -> Vec<String> {
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let base = slugify(&e.text);
        let used = counts.entry(base.clone()).or_insert(0);
        let slug = if *used == 0 {
            base.clone()
        } else {
            format!("{base}-{used}")
        };
        *used += 1;
        out.push(slug);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(level: u8, text: &str, line: u32) -> HeadingEntry {
        HeadingEntry {
            level,
            text: text.into(),
            line,
            start_byte: 0,
        }
    }

    #[test]
    fn slugify_lowercases_and_dashes_spaces() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Foo Bar  Baz"), "foo-bar-baz");
    }

    #[test]
    fn slugify_strips_punctuation_keeps_dashes_and_underscores() {
        assert_eq!(slugify("What's New?"), "whats-new");
        assert_eq!(slugify("snake_case-words"), "snake_case-words");
    }

    #[test]
    fn slugify_trims_leading_trailing_dashes() {
        assert_eq!(slugify("  foo  "), "foo");
        assert_eq!(slugify("--bar--"), "bar");
    }

    #[test]
    fn format_toc_empty_yields_only_markers() {
        let s = format_toc(&[]);
        assert!(s.starts_with(TOC_OPEN_MARKER));
        assert!(s.trim_end().ends_with(TOC_CLOSE_MARKER));
        // Two lines: open + close.
        assert_eq!(s.lines().count(), 2);
    }

    #[test]
    fn format_toc_indents_relative_to_shallowest() {
        let hs = vec![h(2, "Top", 0), h(3, "Mid", 4), h(4, "Deep", 8)];
        let s = format_toc(&hs);
        // Top is at column 0 (relative to shallowest = 2).
        assert!(s.contains("- [Top](#top)"));
        // Mid indented by 2 spaces.
        assert!(s.contains("  - [Mid](#mid)"));
        // Deep indented by 4 spaces.
        assert!(s.contains("    - [Deep](#deep)"));
    }

    #[test]
    fn format_toc_disambiguates_duplicate_slugs() {
        let hs = vec![h(2, "Setup", 0), h(2, "Setup", 8)];
        let s = format_toc(&hs);
        assert!(s.contains("(#setup)"));
        assert!(s.contains("(#setup-1)"));
    }

    #[test]
    fn format_toc_with_levels_clamps_range() {
        let hs = vec![h(1, "One", 0), h(2, "Two", 4), h(3, "Three", 8)];
        let s = format_toc_with_levels(&hs, 2, 3);
        // H1 filtered out.
        assert!(!s.contains("[One]"));
        assert!(s.contains("[Two]"));
        assert!(s.contains("[Three]"));
    }

    #[test]
    fn find_toc_block_returns_range_for_existing_block() {
        let src = "intro\n<!-- toc -->\n- [Foo](#foo)\n<!-- /toc -->\nrest\n";
        let (start, end) = find_toc_block(src).expect("block found");
        assert!(start < end);
        let block = &src[start..end];
        assert!(block.starts_with(TOC_OPEN_MARKER));
        assert!(block.ends_with(TOC_CLOSE_MARKER));
    }

    #[test]
    fn find_toc_block_in_rope_matches_string_range() {
        let src = "intro\n<!-- toc -->\n- [Foo](#foo)\n<!-- /toc -->\nrest\n";
        let rope = ropey::Rope::from_str(src);

        assert_eq!(find_toc_block_in_rope(&rope), find_toc_block(src));
    }

    #[test]
    fn find_toc_block_missing_open_returns_none() {
        let src = "no markers here";
        assert!(find_toc_block(src).is_none());
    }

    #[test]
    fn find_toc_block_missing_close_returns_none() {
        let src = "<!-- toc -->\n- [Foo](#foo)\n";
        assert!(find_toc_block(src).is_none());
    }
}
