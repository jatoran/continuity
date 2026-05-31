//! Tree-sitter markdown parser wrapper.

use tree_sitter::{Parser, Tree};

use crate::Error;

/// Block-level markdown parser. Holds an owned `tree_sitter::Parser` so it
/// can do incremental re-parses against an old tree.
pub struct MarkdownParser {
    parser: Parser,
}

impl MarkdownParser {
    /// Construct a fresh parser configured for the markdown block grammar.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LanguageLoad`] if the bundled grammar can't be applied
    /// (typically only fails when tree-sitter ABI versions mismatch).
    pub fn new() -> Result<Self, Error> {
        // Install the tree-sitter allocation counter before the first
        // `Parser::new()` allocates. This is the sole `Parser` construction
        // site, so this is the one place that satisfies the
        // `tree_sitter_alloc` safety contract. Idempotent (a `Once`).
        crate::tree_sitter_alloc::install();
        let mut parser = Parser::new();
        let language: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
        parser
            .set_language(&language)
            .map_err(|e| Error::LanguageLoad(e.to_string()))?;
        Ok(Self { parser })
    }

    /// Parse `source`, optionally reusing `old_tree` for incremental work.
    pub fn parse(&mut self, source: &str, old_tree: Option<&Tree>) -> Option<Tree> {
        self.parser.parse(source, old_tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_constructs_without_error() {
        let _ = MarkdownParser::new().unwrap();
    }

    #[test]
    fn parses_empty_input() {
        let mut p = MarkdownParser::new().unwrap();
        assert!(p.parse("", None).is_some());
    }

    fn count_kind(tree: &tree_sitter::Tree, kind: &str) -> usize {
        let mut count = 0;
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if node.kind() == kind {
                count += 1;
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
        count
    }

    #[test]
    fn parses_heading_and_paragraph() {
        let mut p = MarkdownParser::new().unwrap();
        let tree = p.parse("# Heading\n\nbody text", None).unwrap();
        assert_eq!(tree.root_node().kind(), "document");
        assert!(count_kind(&tree, "atx_heading") >= 1);
        assert!(count_kind(&tree, "paragraph") >= 1);
    }

    #[test]
    fn incremental_parse_returns_tree() {
        let mut p = MarkdownParser::new().unwrap();
        let t1 = p.parse("# A", None).unwrap();
        let t2 = p.parse("# A\n\nbody", Some(&t1)).unwrap();
        assert!(count_kind(&t2, "atx_heading") >= 1);
        assert!(count_kind(&t2, "paragraph") >= 1);
    }
}
