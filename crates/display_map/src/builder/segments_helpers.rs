//! Supporting pure helpers for [`super::segments::build_line_segments`]:
//! line-local reveal logic, checkbox-reveal slack, the markdown
//! render-toggle emphasis-delimiter book-keeping, and block-level
//! (heading / code) style resolution. Split out of `segments.rs` to keep
//! that file under the 600-line conventions cap. All functions are pure
//! and run on the same worker thread as the builder.

use continuity_decorate::{BlockKind, BlockSpan, Decorations, InlineKind};
use std::ops::Range;

use crate::id::SourceByte;
use crate::markdown_toggles::MarkdownRenderToggles;
use crate::style::{SpanRole, SpanStyle};

fn block_revealed(block: &BlockSpan, caret_bytes: &[SourceByte]) -> bool {
    caret_bytes.iter().any(|c| {
        let b = c.as_usize();
        b >= block.start_byte && b <= block.end_byte
    })
}

pub(super) fn line_revealed(
    decorations: &Decorations,
    caret_bytes: &[SourceByte],
    line_start: usize,
    line_end: usize,
) -> bool {
    // **Reveal is line-local.** A line shows raw markdown when a
    // caret sits on the same source line — nothing else. The
    // previous implementation used byte-range containment against
    // `decorations.blocks`, which had two problems on large
    // documents:
    //
    // 1. **Block flicker.** Block byte ranges shift by exactly the
    //    number of bytes typed when the decoration worker is behind
    //    the rope (a 500 KiB markdown buffer can be several
    //    revisions stale at any instant). A caret near a block
    //    boundary would then flip in-vs-out of the containing block
    //    between consecutive paints — every line in that block
    //    would toggle reveal state, the lines' wrap row counts
    //    would change, and the entire viewport would shift on every
    //    keystroke. The user-visible symptom was "as I type, all of
    //    the markdown rendering across the document toggles live
    //    and not live".
    // 2. **Containment-too-broad.** Even with fresh decorations, the
    //    rule revealed every line in the smallest containing block;
    //    a long paragraph or pipe-table block would still light up
    //    its entire row count when the caret was at a different
    //    offset inside it.
    //
    // The fix avoids both: the check uses *only* rope-derived line
    // bounds and caret bytes, so it's invariant under stale
    // decorations and tight to the single source line the caret is
    // on. Multi-line code blocks remain the one exception — those
    // reveal as a unit so the fence markers and indented body show
    // when the caret is inside, which is essential UX for editing
    // code in-place. Code-block boundaries are whole-line so
    // off-by-N-byte staleness can't flip individual lines in or out
    // of the block.
    if caret_bytes
        .iter()
        .any(|c| c.as_usize() >= line_start && c.as_usize() <= line_end)
    {
        return true;
    }
    for block in &decorations.blocks {
        if block.start_byte >= line_end || block.end_byte <= line_start {
            continue;
        }
        let reveals_as_unit = matches!(
            block.kind,
            BlockKind::FencedCodeBlock | BlockKind::IndentedCodeBlock
        );
        if reveals_as_unit && block_revealed(block, caret_bytes) {
            return true;
        }
    }
    false
}

/// Bytes of slack on either side of a checkbox span within which a caret
/// reveals the raw `[ ]` brackets.
const CHECKBOX_REVEAL_SLACK_BYTES: usize = 2;

/// Unlike the rest of a markdown line, a task checkbox reveals its raw
/// brackets only when a caret sits *on or beside the checkbox itself*,
/// not merely somewhere on the line. This lets the writer keep editing
/// the line's text with the checkbox glyph still showing, yet still edit
/// (or delete) the brackets when the caret is near them.
///
/// `span_start..span_end` is the checkbox span clamped to the line; the
/// reveal window extends [`CHECKBOX_REVEAL_SLACK_BYTES`] past each edge,
/// clamped to the line so an off-line caret never reveals it.
pub(super) fn caret_near_checkbox(
    caret_bytes: &[SourceByte],
    span_start: usize,
    span_end: usize,
    line_start: usize,
    line_end: usize,
) -> bool {
    let lo = span_start
        .saturating_sub(CHECKBOX_REVEAL_SLACK_BYTES)
        .max(line_start);
    let hi = span_end
        .saturating_add(CHECKBOX_REVEAL_SLACK_BYTES)
        .min(line_end);
    caret_bytes.iter().any(|c| {
        let pos = c.as_usize();
        pos >= lo && pos <= hi
    })
}

/// Collect the line-clamped body byte ranges of every `Emphasis` /
/// `Strong` inline span whose render toggle is OFF and that intersects
/// `[line_start, line_end]`. Their `*` / `**` delimiters must stay
/// visible (literal markup) rather than being hidden, so
/// [`super::segments::build_line_segments`] consults this set in the
/// `EmphasisDelim` arm. Returns an empty `Vec` when both `italic` and
/// `bold` are on (the common path), so the adjacency check is skipped.
pub(super) fn collect_toggle_off_emphasis_ranges(
    decorations: &Decorations,
    toggles: MarkdownRenderToggles,
    line_start: usize,
    line_end: usize,
) -> Vec<Range<usize>> {
    if toggles.italic && toggles.bold {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    for span in &decorations.inlines {
        let toggled_off = match span.kind {
            InlineKind::Emphasis => !toggles.italic,
            InlineKind::Strong => !toggles.bold,
            _ => false,
        };
        if !toggled_off {
            continue;
        }
        let s = span.range.start.max(line_start);
        let e = span.range.end.min(line_end);
        if s < e {
            ranges.push(s..e);
        }
    }
    ranges
}

/// `true` when the delimiter byte range `[delim_start, delim_end]`
/// abuts or overlaps any kept emphasis/strong body range. A `*foo*`
/// delimiter sits immediately outside the styled body, so adjacency
/// (touching at an endpoint) is the relevant test, not strict overlap.
pub(super) fn range_touches_kept(
    kept: &[Range<usize>],
    delim_start: usize,
    delim_end: usize,
) -> bool {
    kept.iter()
        .any(|body| delim_start <= body.end && body.start <= delim_end)
}

/// Map a footnote label's ASCII digits to their superscript glyphs for
/// the collapsed `[^1]` → `¹` reference replacement.
pub(super) fn superscript_label(label: &str) -> String {
    let mut out = String::new();
    for ch in label.chars() {
        out.push(match ch {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            other => other,
        });
    }
    out
}

/// Resolve the block-level [`SpanStyle`] for a line: heading scaling
/// (ATX always; setext gated by `render_setext_heading`) and fenced /
/// indented code-block role. Inline emphasis merges on top of this in
/// the caller.
pub(super) fn compute_block_style(
    decorations: &Decorations,
    line_start: usize,
    line_end: usize,
    is_line_revealed: bool,
    toggles: MarkdownRenderToggles,
) -> SpanStyle {
    let mut style = SpanStyle::body();
    for b in &decorations.blocks {
        if b.start_byte >= line_end || b.end_byte <= line_start {
            continue;
        }
        match b.kind {
            // ATX `#` headings always honour heading styling; setext
            // (`===` / `---` underline) headings are gated by
            // `render_setext_heading`. When OFF the heading text renders
            // unscaled body text and the underline row stays literal.
            BlockKind::SetextHeading { .. } if !toggles.setext_heading => {}
            BlockKind::Heading { level } | BlockKind::SetextHeading { level } => {
                let lvl = level.clamp(1, 6);
                let heading_style = if is_line_revealed {
                    SpanStyle::heading_revealed(lvl)
                } else {
                    SpanStyle::heading(lvl)
                };
                style = style.merge(heading_style);
            }
            BlockKind::FencedCodeBlock => {
                style = style.merge(SpanStyle {
                    role: SpanRole::Code,
                    ..SpanStyle::body()
                });
            }
            // IndentedCodeBlock is CommonMark's implicit "4-space-prefix
            // is code" rule — too aggressive for a notes editor where
            // indented prose is just prose. Leave the text style at body.
            BlockKind::IndentedCodeBlock => {}
            _ => {}
        }
    }
    style
}
