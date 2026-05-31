//! Walking a parsed markdown tree to extract block-level structural spans.

use tree_sitter::Tree;

/// What kind of block a `BlockSpan` describes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// `# heading` through `###### heading` (level = 1..=6).
    Heading {
        /// 1..=6.
        level: u8,
    },
    /// `setext` heading (`Title\n=====` / `-----`).
    SetextHeading {
        /// 1 (`=`) or 2 (`-`).
        level: u8,
    },
    /// A paragraph.
    Paragraph,
    /// A fenced code block (` ``` ` or `~~~`).
    FencedCodeBlock,
    /// An indented code block (4-space prefix).
    IndentedCodeBlock,
    /// An unordered or ordered list (the wrapping list, not items).
    List,
    /// A single list item.
    ListItem,
    /// `> blockquote`.
    BlockQuote,
    /// `---` / `***` horizontal rule.
    HorizontalRule,
    /// A pipe table.
    PipeTable,
    /// HTML block.
    HtmlBlock,
    /// Anything tree-sitter labeled but we don't categorize.
    Other(&'static str),
}

/// A block span: kind + byte range in source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSpan {
    /// What kind of block.
    pub kind: BlockKind,
    /// Inclusive start byte.
    pub start_byte: usize,
    /// Exclusive end byte.
    pub end_byte: usize,
}

/// Walk `tree` and collect block-level children, transparently descending
/// through `section` wrapper nodes that `tree-sitter-md` introduces.
#[must_use]
pub fn block_spans(tree: &Tree) -> Vec<BlockSpan> {
    let root = tree.root_node();
    let mut out = Vec::new();
    collect(root, &mut out);
    out
}

fn collect<'a>(node: tree_sitter::Node<'a>, out: &mut Vec<BlockSpan>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "section" {
            collect(child, out);
            continue;
        }
        let kind = classify(child.kind(), child.child_count(), child);
        out.push(BlockSpan {
            kind,
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
        });
    }
}

fn classify(node_kind: &str, _child_count: usize, node: tree_sitter::Node<'_>) -> BlockKind {
    match node_kind {
        "atx_heading" => BlockKind::Heading {
            level: atx_level(node),
        },
        "setext_heading" => BlockKind::SetextHeading {
            level: setext_level(node),
        },
        "paragraph" => BlockKind::Paragraph,
        "fenced_code_block" => BlockKind::FencedCodeBlock,
        "indented_code_block" => BlockKind::IndentedCodeBlock,
        "list" => BlockKind::List,
        "list_item" => BlockKind::ListItem,
        "block_quote" => BlockKind::BlockQuote,
        "thematic_break" => BlockKind::HorizontalRule,
        "pipe_table" => BlockKind::PipeTable,
        "html_block" => BlockKind::HtmlBlock,
        // `tree-sitter-md` labels each ATX heading marker by its hash count.
        // Map static lifetimes through a small set of literals.
        other => static_other(other),
    }
}

fn atx_level(node: tree_sitter::Node<'_>) -> u8 {
    // Find the marker child like "atx_h1_marker" through "atx_h6_marker".
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(level) = child
            .kind()
            .strip_prefix("atx_h")
            .and_then(|s| s.strip_suffix("_marker"))
        {
            if let Ok(n) = level.parse::<u8>() {
                return n;
            }
        }
    }
    1
}

fn setext_level(node: tree_sitter::Node<'_>) -> u8 {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "setext_h1_underline" => return 1,
            "setext_h2_underline" => return 2,
            _ => {}
        }
    }
    1
}

fn static_other(s: &str) -> BlockKind {
    // Map common kinds to &'static str literals so BlockKind::Other carries a
    // stable string. Anything unrecognized becomes "unknown".
    match s {
        "section" => BlockKind::Other("section"),
        "minus_metadata" => BlockKind::Other("minus_metadata"),
        "plus_metadata" => BlockKind::Other("plus_metadata"),
        "link_reference_definition" => BlockKind::Other("link_reference_definition"),
        _ => BlockKind::Other("unknown"),
    }
}

#[cfg(test)]
mod tests {
    use crate::MarkdownParser;

    use super::*;

    fn spans(src: &str) -> Vec<BlockSpan> {
        let mut p = MarkdownParser::new().unwrap();
        let tree = p.parse(src, None).unwrap();
        block_spans(&tree)
    }

    #[test]
    fn empty_source_has_no_blocks() {
        assert!(spans("").is_empty());
    }

    #[test]
    fn single_paragraph() {
        let s = spans("hello world");
        assert_eq!(s.len(), 1);
        assert!(matches!(
            s[0].kind,
            BlockKind::Paragraph | BlockKind::Other(_)
        ));
    }

    #[test]
    fn heading_levels_extracted() {
        let s = spans("# h1\n\n## h2\n\n### h3\n");
        let levels: Vec<u8> = s
            .iter()
            .filter_map(|b| match b.kind {
                BlockKind::Heading { level } => Some(level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 3]);
    }

    #[test]
    fn fenced_code_block_recognized() {
        let s = spans("```rust\nfn main() {}\n```\n");
        assert!(s
            .iter()
            .any(|b| matches!(b.kind, BlockKind::FencedCodeBlock)));
    }

    #[test]
    fn block_quote_recognized() {
        let s = spans("> quoted\n");
        assert!(s.iter().any(|b| matches!(b.kind, BlockKind::BlockQuote)));
    }
}
