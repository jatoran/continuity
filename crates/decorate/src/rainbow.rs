//! Phase B8 rainbow bracket-pair highlighting.
//!
//! Pure function that walks source text and tags every `()`, `[]`,
//! `{}` bracket with its nesting depth. Brackets inside markdown code
//! spans / code fences are not depth-tracked — the source is treated
//! as opaque text inside fences (the existing decoration pass already
//! marks those regions and the renderer can skip rainbow tints there
//! when the time comes).
//!
//! The rainbow render is a glyph-color override at paint time, not a
//! source mutation — this module just produces the byte-range +
//! depth-index pairs the renderer needs.

use std::ops::Range;

/// One rainbow-coloured bracket at a specific source byte. The renderer
/// indexes into the theme's `editor.pair_rainbow.N` palette using
/// `depth_index % palette_size`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BracketDepth {
    /// Byte offset of this bracket character.
    pub byte: usize,
    /// 0-based nesting depth from the outermost enclosing pair.
    /// The first `(` in `((x))` is `0`, the inner `(` is `1`.
    pub depth: u32,
    /// `true` when this bracket is an opening character (`(`, `[`, `{`).
    pub opening: bool,
}

/// Walk `text` and return per-bracket depth tags.
///
/// Mismatched / unbalanced brackets are tagged with their *current*
/// depth at the byte they appear; depth never goes negative
/// (saturating_sub on close). String / comment / code-fence skipping
/// is out of scope here — markdown's tree-sitter pass labels those
/// regions separately.
#[must_use]
pub fn bracket_depths(text: &str) -> Vec<BracketDepth> {
    let mut out = Vec::new();
    let mut stack: Vec<u8> = Vec::new();
    for (byte, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => {
                let depth = stack.len() as u32;
                out.push(BracketDepth {
                    byte,
                    depth,
                    opening: true,
                });
                stack.push(ch as u8);
            }
            ')' | ']' | '}' => {
                // Pattern guard above guarantees ch is one of `) ] }`.
                let matching: u8 = if ch == ')' {
                    b'('
                } else if ch == ']' {
                    b'['
                } else {
                    b'{'
                };
                while let Some(&top) = stack.last() {
                    if top == matching {
                        stack.pop();
                        break;
                    }
                    // Mismatched close: drop the unmatched open and
                    // keep walking — keeps depth bounded.
                    stack.pop();
                }
                let depth = stack.len() as u32;
                out.push(BracketDepth {
                    byte,
                    depth,
                    opening: false,
                });
            }
            _ => {}
        }
    }
    out
}

/// Convenience: same data as [`bracket_depths`] folded into half-open
/// byte ranges of length 1 (every bracket is exactly one byte for the
/// three ASCII pair kinds). Saves the renderer one allocation.
#[must_use]
pub fn bracket_ranges(text: &str) -> Vec<(Range<usize>, u32)> {
    bracket_depths(text)
        .into_iter()
        .map(|b| (b.byte..b.byte + 1, b.depth))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_pairs_get_depth_zero() {
        let bs = bracket_depths("(a) [b] {c}");
        assert_eq!(bs.len(), 6);
        assert!(bs.iter().all(|b| b.depth == 0));
    }

    #[test]
    fn nested_pairs_increment_depth() {
        let bs = bracket_depths("((x))");
        assert_eq!(bs.len(), 4);
        assert_eq!(bs[0].depth, 0);
        assert!(bs[0].opening);
        assert_eq!(bs[1].depth, 1);
        assert!(bs[1].opening);
        assert_eq!(bs[2].depth, 1);
        assert!(!bs[2].opening);
        assert_eq!(bs[3].depth, 0);
    }

    #[test]
    fn mixed_pair_kinds_share_depth() {
        // The depth counter is *kind-agnostic* by design — match the
        // VSCode rainbow-brackets convention.
        let bs = bracket_depths("([{}])");
        assert_eq!(
            bs.iter().map(|b| b.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 2, 1, 0]
        );
    }

    #[test]
    fn mismatched_close_clears_stack_to_match() {
        let bs = bracket_depths("({)}");
        // `(` 0 → push `(`. `{` 1 → push `{`. `)` searches for `(`,
        // pops `{` and `(` to match — depth at the `)` is 0 (after
        // the pops). `}` finds no matching `{` → stack empty,
        // depth 0.
        assert_eq!(bs[0].depth, 0);
        assert_eq!(bs[1].depth, 1);
        assert_eq!(bs[2].depth, 0);
        assert_eq!(bs[3].depth, 0);
    }

    #[test]
    fn no_brackets_returns_empty() {
        assert!(bracket_depths("plain text").is_empty());
    }

    #[test]
    fn unbalanced_close_does_not_underflow() {
        // Lone close — depth stays 0.
        let bs = bracket_depths(")))");
        assert!(bs.iter().all(|b| b.depth == 0 && !b.opening));
    }

    #[test]
    fn unicode_text_does_not_break_byte_offsets() {
        let bs = bracket_depths("(α)");
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].byte, 0);
        // `α` is 2 bytes (U+03B1), so `)` sits at byte 3.
        assert_eq!(bs[1].byte, 3);
    }

    #[test]
    fn bracket_ranges_emits_length_one_spans() {
        let rs = bracket_ranges("()");
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0], (0..1, 0));
        assert_eq!(rs[1], (1..2, 0));
    }
}
