//! Block-level renderers for the HTML-to-markdown converter: fenced code,
//! lists, blockquotes, and the `<table>` wrapper.
//!
//! Split out of [`crate::html_to_markdown`] to keep that module under the
//! 600-line cap. These functions share the parent's `MarkdownWriter` /
//! `RenderContext` and recurse back into the parent's node renderers.

use crate::clipboard_html::{render_table, HtmlNode};
use crate::html_to_markdown::{
    code_block_fence, collapse_whitespace, render_inline_children, render_inline_to_string,
    render_node, render_nodes, MarkdownWriter, RenderContext,
};

/// Render `<pre>` (and `<pre><code class="language-…">`) as a fenced code
/// block, preserving interior whitespace and choosing a language hint
/// from a child `<code>`'s class when present.
pub(crate) fn render_pre(children: &[HtmlNode], writer: &mut MarkdownWriter, ctx: &RenderContext) {
    let (lang, body) = pre_body_and_language(children, ctx);
    let body = body.trim_matches('\n');
    writer.ensure_block_break();
    let fence = code_block_fence(body);
    writer.push_inline(&fence);
    writer.push_inline(lang.as_deref().unwrap_or(""));
    writer.push_inline("\n");
    writer.push_inline(body);
    writer.push_inline("\n");
    writer.push_inline(&fence);
    writer.ensure_block_break();
}

/// Extract a `<pre>`'s code body and an optional language hint from a
/// child `<code class="language-xxx">`.
fn pre_body_and_language(children: &[HtmlNode], ctx: &RenderContext) -> (Option<String>, String) {
    let pre_ctx = ctx.preformatted();
    for child in children {
        if let HtmlNode::Element {
            tag,
            attrs,
            children: code_children,
        } = child
        {
            if tag == "code" {
                let lang = attrs
                    .get("class")
                    .and_then(|class| language_from_class(class));
                let body = render_text_to_string(code_children, &pre_ctx);
                return (lang, body);
            }
        }
    }
    (None, render_text_to_string(children, &pre_ctx))
}

/// Pull a language token out of a `class="language-rust highlight"`-style
/// attribute. Recognizes both `language-xxx` and `lang-xxx` prefixes.
fn language_from_class(class: &str) -> Option<String> {
    for token in class.split_whitespace() {
        if let Some(lang) = token.strip_prefix("language-") {
            if !lang.is_empty() {
                return Some(lang.to_string());
            }
        }
        if let Some(lang) = token.strip_prefix("lang-") {
            if !lang.is_empty() {
                return Some(lang.to_string());
            }
        }
    }
    None
}

/// Distinguishes bullet vs. ordered lists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKind {
    Unordered,
    Ordered,
}

/// Render a `<ul>`/`<ol>` and its `<li>` children with proper nesting
/// indentation.
pub(crate) fn render_list(
    children: &[HtmlNode],
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
    kind: ListKind,
) {
    let nested = ctx.list_depth() > 0;
    // A top-level list opens its own block (blank line before); a nested
    // list sits directly on the line after its parent item's text.
    if nested {
        writer.ensure_line_break();
    } else {
        writer.ensure_block_break();
    }
    let indent = "  ".repeat(ctx.list_depth());
    let item_ctx = ctx.deeper_list();
    let mut ordinal = 1usize;
    for child in children {
        if let HtmlNode::Element {
            tag,
            children: li_children,
            ..
        } = child
        {
            if tag != "li" {
                continue;
            }
            writer.ensure_line_break();
            let marker = match kind {
                ListKind::Unordered => "- ".to_string(),
                ListKind::Ordered => format!("{ordinal}. "),
            };
            writer.push_inline(&indent);
            writer.push_inline(&marker);
            render_list_item(li_children, writer, &item_ctx);
            ordinal += 1;
        }
    }
    if !nested {
        writer.ensure_block_break();
    }
}

/// Render a list item's children: inline content stays on the marker
/// line; nested lists / block children are emitted after a line break.
fn render_list_item(children: &[HtmlNode], writer: &mut MarkdownWriter, ctx: &RenderContext) {
    for child in children {
        match child {
            HtmlNode::Element { tag, .. } if matches!(tag.as_str(), "ul" | "ol") => {
                render_node(child, writer, ctx);
            }
            HtmlNode::Element {
                tag,
                children: inner,
                ..
            } if tag == "p" => {
                // Treat a paragraph inside a list item as inline so the
                // text stays on the marker line.
                render_inline_children(inner, writer, ctx);
            }
            _ => render_node(child, writer, ctx),
        }
    }
}

/// Render `<blockquote>` content with `> ` prefixes applied to each line.
pub(crate) fn render_blockquote(
    children: &[HtmlNode],
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
) {
    let inner = {
        // Render the quote body into a sub-writer, then prefix each line
        // with `> `. Nested blockquotes prefix again at each level, so no
        // explicit depth counter is needed.
        let mut inner_writer = MarkdownWriter::default();
        render_nodes(children, &mut inner_writer, ctx);
        inner_writer.finish()
    };
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return;
    }
    writer.ensure_block_break();
    for line in trimmed.lines() {
        if line.is_empty() {
            writer.push_inline(">");
        } else {
            writer.push_inline("> ");
            writer.push_inline(line);
        }
        writer.push_inline("\n");
    }
    writer.ensure_block_break();
}

/// Render a `<table>` element as a GFM pipe table. The grid extraction and
/// formatting live in [`crate::clipboard_html`]; the inline content of
/// each cell is rendered via the parent so emphasis / links / code inside
/// cells survive.
pub(crate) fn render_table_block(
    node: &HtmlNode,
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
) {
    let cell_ctx = ctx.clone();
    let rendered = render_table(node, &mut |cell_children| {
        // Cells are inline contexts; collapse to a single line so a `<br>`
        // or block child can't break the pipe row.
        let raw = render_inline_to_string(cell_children, &cell_ctx);
        raw.replace('\n', " ").trim().to_string()
    });
    let Some(table) = rendered else {
        // Not a usable table — fall back to rendering its text children.
        if let HtmlNode::Element { children, .. } = node {
            render_nodes(children, writer, ctx);
        }
        return;
    };
    writer.ensure_block_break();
    writer.push_inline(&table);
    writer.ensure_block_break();
}

/// Render only the text content of `children` (no markdown formatting),
/// honoring the preformatted flag in `ctx`. Used for inline + fenced code.
pub(crate) fn render_text_to_string(children: &[HtmlNode], ctx: &RenderContext) -> String {
    let mut out = String::new();
    collect_text(children, ctx, &mut out);
    out
}

/// Recursively collect text, converting `<br>` to newlines and treating
/// block-level children as line breaks.
fn collect_text(nodes: &[HtmlNode], ctx: &RenderContext, out: &mut String) {
    for node in nodes {
        match node {
            HtmlNode::Text(text) => {
                if ctx.is_preformatted() {
                    out.push_str(text);
                } else {
                    out.push_str(&collapse_whitespace(text));
                }
            }
            HtmlNode::Element { tag, children, .. } => match tag.as_str() {
                "br" => out.push('\n'),
                "p" | "div" => {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    collect_text(children, ctx, out);
                }
                _ => collect_text(children, ctx, out),
            },
        }
    }
}
