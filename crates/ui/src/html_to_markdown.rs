//! Convert clipboard HTML (the `"HTML Format"` payload) into markdown for
//! paste ([`crate::window_clipboard`]).
//!
//! The DOM is built by the dependency-free parser in
//! [`crate::clipboard_html`]; this module walks that tree and renders a
//! markdown approximation. Coverage (per Phase-D paste-richness item 16):
//!
//! * `a` → `[text](href)` (bare text when no usable href)
//! * `img` → `![alt](src)`
//! * `b` / `strong` → `**text**`
//! * `i` / `em` → `*text*`
//! * `code` (inline) → `` `text` ``
//! * `pre` (and `pre > code`) → fenced ```` ``` ```` block
//! * `h1`–`h6` → `#`..`######` headings
//! * `ul` / `ol` / `li` → bullet / ordered lists (nested-aware)
//! * `blockquote` → `> ` quote
//! * `br` → hard line break; `p` / `div` → blank-line-separated blocks
//! * `table` / `tr` / `th` / `td` → GFM pipe table
//! * `s` / `del` / `strike` → `~~text~~`
//!
//! The output is conservative: when an element has no markdown meaning we
//! still render its children, so no text is ever dropped. Whitespace is
//! collapsed the way a browser would for inline content, then trimmed at
//! block boundaries.
//!
//! Thread ownership: pure functions, called on the UI thread from the
//! paste path.

use crate::clipboard_html::{parse_html, HtmlNode};

mod blocks;

/// Convert a CF_HTML fragment string into markdown.
///
/// Returns `None` when the input has no usable content (so the paste path
/// can fall back to plain text). The returned string has trailing/leading
/// blank lines trimmed and uses `\n` line endings only.
#[must_use]
pub(crate) fn html_to_markdown(html: &str) -> Option<String> {
    let nodes = parse_html(html);
    let mut writer = MarkdownWriter::default();
    render_nodes(&nodes, &mut writer, &RenderContext::default());
    let output = writer.finish();
    let trimmed = output.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Ambient state threaded through the recursive render: list nesting,
/// ordered-list counters, and whether we are inside a quote / preformatted
/// context (which changes whitespace handling).
#[derive(Clone, Default)]
pub(crate) struct RenderContext {
    /// List nesting depth (controls indentation of list items).
    list_depth: usize,
    /// `true` inside `<pre>` — whitespace and newlines are preserved.
    preformatted: bool,
}

impl RenderContext {
    pub(crate) fn deeper_list(&self) -> Self {
        let mut next = self.clone();
        next.list_depth += 1;
        next
    }
    pub(crate) fn preformatted(&self) -> Self {
        let mut next = self.clone();
        next.preformatted = true;
        next
    }
    pub(crate) fn is_preformatted(&self) -> bool {
        self.preformatted
    }
    pub(crate) fn list_depth(&self) -> usize {
        self.list_depth
    }
}

/// Accumulates markdown output with helpers for block separation and
/// quote-prefix handling.
#[derive(Default)]
pub(crate) struct MarkdownWriter {
    /// The rendered markdown built so far.
    out: String,
}

impl MarkdownWriter {
    /// Append inline text verbatim (no separator logic).
    pub(crate) fn push_inline(&mut self, text: &str) {
        self.out.push_str(text);
    }

    /// Ensure the buffer ends with a blank line (block boundary), unless
    /// it is empty.
    pub(crate) fn ensure_block_break(&mut self) {
        if self.out.is_empty() {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if self.out.ends_with("\n\n") {
            return;
        }
        if self.out.ends_with('\n') {
            self.out.push('\n');
        } else {
            self.out.push_str("\n\n");
        }
    }

    /// Ensure the buffer ends with a single newline (line boundary).
    pub(crate) fn ensure_line_break(&mut self) {
        if self.out.is_empty() {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    pub(crate) fn finish(self) -> String {
        self.out
    }
}

/// Render a list of sibling nodes.
pub(crate) fn render_nodes(nodes: &[HtmlNode], writer: &mut MarkdownWriter, ctx: &RenderContext) {
    for node in nodes {
        render_node(node, writer, ctx);
    }
}

/// Render a single node.
pub(crate) fn render_node(node: &HtmlNode, writer: &mut MarkdownWriter, ctx: &RenderContext) {
    match node {
        HtmlNode::Text(text) => {
            let rendered = if ctx.preformatted {
                text.clone()
            } else {
                collapse_whitespace(text)
            };
            if !rendered.is_empty() {
                writer.push_inline(&escape_markdown_inline(&rendered, ctx.preformatted));
            }
        }
        HtmlNode::Element { tag, children, .. } => render_element(tag, node, children, writer, ctx),
    }
}

/// Render an element by tag name.
fn render_element(
    tag: &str,
    node: &HtmlNode,
    children: &[HtmlNode],
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
) {
    match tag {
        "br" => writer.ensure_line_break(),
        "hr" => {
            writer.ensure_block_break();
            writer.push_inline("---");
            writer.ensure_block_break();
        }
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "figure" => {
            writer.ensure_block_break();
            render_nodes(children, writer, ctx);
            writer.ensure_block_break();
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = (tag.as_bytes()[1] - b'0') as usize;
            writer.ensure_block_break();
            writer.push_inline(&"#".repeat(level));
            writer.push_inline(" ");
            render_inline_children(children, writer, ctx);
            writer.ensure_block_break();
        }
        "b" | "strong" => render_wrapped(children, writer, ctx, "**", "**"),
        "i" | "em" => render_wrapped(children, writer, ctx, "*", "*"),
        "s" | "del" | "strike" => render_wrapped(children, writer, ctx, "~~", "~~"),
        "u" => render_inline_children(children, writer, ctx),
        "code" => render_inline_code(children, writer, ctx),
        "pre" => blocks::render_pre(children, writer, ctx),
        "a" => render_anchor(node, children, writer, ctx),
        "img" => render_image(node, writer),
        "ul" => blocks::render_list(children, writer, ctx, blocks::ListKind::Unordered),
        "ol" => blocks::render_list(children, writer, ctx, blocks::ListKind::Ordered),
        "blockquote" => blocks::render_blockquote(children, writer, ctx),
        "table" => blocks::render_table_block(node, writer, ctx),
        // Containers we don't model: render children transparently.
        _ => render_nodes(children, writer, ctx),
    }
}

/// Render children that are expected to be inline, collapsing surrounding
/// whitespace into the flow.
pub(crate) fn render_inline_children(
    children: &[HtmlNode],
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
) {
    render_nodes(children, writer, ctx);
}

/// Wrap the rendered inline content of `children` in `open`/`close`,
/// taking care not to emit empty emphasis (`****`).
fn render_wrapped(
    children: &[HtmlNode],
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
    open: &str,
    close: &str,
) {
    let inner = render_inline_to_string(children, ctx);
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        // Preserve any whitespace so words don't run together.
        if !inner.is_empty() {
            writer.push_inline(" ");
        }
        return;
    }
    // Preserve leading/trailing spaces outside the markers so emphasis
    // attaches to the word, matching CommonMark's flanking rules.
    let leading = if inner.starts_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    let trailing = if inner.ends_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    writer.push_inline(leading);
    writer.push_inline(open);
    writer.push_inline(trimmed);
    writer.push_inline(close);
    writer.push_inline(trailing);
}

/// Render inline `<code>` as `` `text` ``. Uses enough backticks to wrap
/// any literal backticks the content contains.
fn render_inline_code(children: &[HtmlNode], writer: &mut MarkdownWriter, ctx: &RenderContext) {
    // Inside inline code, treat content as preformatted so entities are
    // preserved literally and no markdown escaping is applied.
    let raw = blocks::render_text_to_string(children, &ctx.preformatted());
    let trimmed = raw.trim_matches('\n');
    if trimmed.is_empty() {
        return;
    }
    let fence = backtick_fence(trimmed);
    writer.push_inline(&fence);
    if trimmed.starts_with('`') || trimmed.ends_with('`') {
        writer.push_inline(" ");
        writer.push_inline(trimmed);
        writer.push_inline(" ");
    } else {
        writer.push_inline(trimmed);
    }
    writer.push_inline(&fence);
}

/// Render `<a href>` as `[text](href)`. Falls back to plain text when the
/// href is missing, empty, or a `javascript:`/anchor-only link, and to the
/// href itself when the link has no visible text.
fn render_anchor(
    node: &HtmlNode,
    children: &[HtmlNode],
    writer: &mut MarkdownWriter,
    ctx: &RenderContext,
) {
    let href = node.attr("href").map(str::trim).unwrap_or("");
    let text = render_inline_to_string(children, ctx);
    let text_trimmed = text.trim();
    let usable_href = !href.is_empty()
        && !href.starts_with('#')
        && !href.to_ascii_lowercase().starts_with("javascript:");
    if !usable_href {
        writer.push_inline(text_trimmed);
        return;
    }
    let label = if text_trimmed.is_empty() {
        href
    } else {
        text_trimmed
    };
    writer.push_inline("[");
    writer.push_inline(label);
    writer.push_inline("](");
    writer.push_inline(&encode_link_target(href));
    writer.push_inline(")");
}

/// Render `<img>` as `![alt](src)`. Emits nothing when there is no source.
fn render_image(node: &HtmlNode, writer: &mut MarkdownWriter) {
    let src = node.attr("src").map(str::trim).unwrap_or("");
    if src.is_empty() {
        return;
    }
    let alt = node
        .attr("alt")
        .map(str::trim)
        .unwrap_or("")
        .replace(['[', ']'], "");
    writer.push_inline("![");
    writer.push_inline(&alt);
    writer.push_inline("](");
    writer.push_inline(&encode_link_target(src));
    writer.push_inline(")");
}

/// Render the inline markdown of `children` to an owned string (used for
/// emphasis wrapping, anchor labels, and table cells).
pub(crate) fn render_inline_to_string(children: &[HtmlNode], ctx: &RenderContext) -> String {
    let mut writer = MarkdownWriter::default();
    render_nodes(children, &mut writer, ctx);
    writer.finish()
}

/// Collapse runs of ASCII whitespace into single spaces (browser inline
/// behavior). Non-breaking spaces are preserved.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch == '\u{00A0}' {
            out.push(ch);
            last_was_space = false;
        } else if ch.is_ascii_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out
}

/// Escape markdown-significant characters in plain text so pasted prose
/// doesn't accidentally form markdown syntax. Skips escaping inside
/// preformatted contexts.
fn escape_markdown_inline(text: &str, preformatted: bool) -> String {
    if preformatted {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' | '`' | '*' | '_' | '[' | ']' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Encode the few characters that break markdown's `(...)` link-target
/// syntax. Spaces and parentheses are percent-encoded; everything else
/// passes through verbatim.
fn encode_link_target(url: &str) -> String {
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// Choose a backtick run long enough to wrap inline code containing
/// literal backticks (CommonMark inline-code fencing rule).
fn backtick_fence(text: &str) -> String {
    "`".repeat(longest_backtick_run(text) + 1)
}

/// Choose a fence of at least three backticks for a fenced code block,
/// longer if the body itself contains a long backtick run.
pub(crate) fn code_block_fence(body: &str) -> String {
    "`".repeat(longest_backtick_run(body).max(2) + 1)
}

/// Length of the longest consecutive run of backtick characters in `text`.
fn longest_backtick_run(text: &str) -> usize {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    max_run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        html_to_markdown(html).unwrap_or_default()
    }

    #[test]
    fn anchor_to_link() {
        assert_eq!(
            md(r#"<a href="https://x.y">click</a>"#),
            "[click](https://x.y)"
        );
    }

    #[test]
    fn anchor_without_href_is_plain() {
        assert_eq!(md(r#"<a>plain</a>"#), "plain");
    }

    #[test]
    fn anchor_url_with_spaces_encoded() {
        assert_eq!(md(r#"<a href="a b">t</a>"#), "[t](a%20b)");
    }

    #[test]
    fn image_to_markdown() {
        assert_eq!(md(r#"<img src="u.png" alt="cat">"#), "![cat](u.png)");
    }

    #[test]
    fn bold_and_italic() {
        assert_eq!(md("<b>x</b>"), "**x**");
        assert_eq!(md("<strong>y</strong>"), "**y**");
        assert_eq!(md("<i>x</i>"), "*x*");
        assert_eq!(md("<em>y</em>"), "*y*");
    }

    #[test]
    fn strikethrough() {
        assert_eq!(md("<del>x</del>"), "~~x~~");
        assert_eq!(md("<s>y</s>"), "~~y~~");
    }

    #[test]
    fn inline_code() {
        assert_eq!(md("<code>let x</code>"), "`let x`");
    }

    #[test]
    fn inline_code_with_backtick() {
        assert_eq!(md("<code>a`b</code>"), "``a`b``");
    }

    #[test]
    fn fenced_code_block() {
        let out = md("<pre><code>line1\nline2</code></pre>");
        assert_eq!(out, "```\nline1\nline2\n```");
    }

    #[test]
    fn fenced_code_block_with_language() {
        let out = md(r#"<pre><code class="language-rust">fn main(){}</code></pre>"#);
        assert_eq!(out, "```rust\nfn main(){}\n```");
    }

    #[test]
    fn headings() {
        assert_eq!(md("<h1>Title</h1>"), "# Title");
        assert_eq!(md("<h3>Sub</h3>"), "### Sub");
    }

    #[test]
    fn unordered_list() {
        let out = md("<ul><li>a</li><li>b</li></ul>");
        assert_eq!(out, "- a\n- b");
    }

    #[test]
    fn ordered_list() {
        let out = md("<ol><li>a</li><li>b</li></ol>");
        assert_eq!(out, "1. a\n2. b");
    }

    #[test]
    fn nested_list() {
        let out = md("<ul><li>a<ul><li>b</li></ul></li></ul>");
        assert_eq!(out, "- a\n  - b");
    }

    #[test]
    fn blockquote() {
        let out = md("<blockquote><p>quoted</p></blockquote>");
        assert_eq!(out, "> quoted");
    }

    #[test]
    fn paragraphs_separated_by_blank_line() {
        let out = md("<p>one</p><p>two</p>");
        assert_eq!(out, "one\n\ntwo");
    }

    #[test]
    fn br_is_line_break() {
        let out = md("a<br>b");
        assert_eq!(out, "a\nb");
    }

    #[test]
    fn whitespace_collapsed() {
        let out = md("<p>a    b\n  c</p>");
        assert_eq!(out, "a b c");
    }

    #[test]
    fn entities_decoded() {
        assert_eq!(md("<p>a &amp; b &lt;c&gt;</p>"), "a & b <c>");
    }

    #[test]
    fn plain_text_escaped() {
        // Asterisks in prose become literal, not emphasis.
        assert_eq!(md("<p>2 * 3</p>"), "2 \\* 3");
    }

    #[test]
    fn empty_html_returns_none() {
        assert!(html_to_markdown("").is_none());
        assert!(html_to_markdown("<p>   </p>").is_none());
        assert!(html_to_markdown("<!-- comment -->").is_none());
    }

    #[test]
    fn unknown_container_renders_children() {
        assert_eq!(md("<span>kept</span>"), "kept");
    }

    #[test]
    fn mixed_inline_formatting() {
        let out = md("<p>Hello <b>bold</b> and <i>italic</i>.</p>");
        assert_eq!(out, "Hello **bold** and *italic*.");
    }

    #[test]
    fn table_to_gfm_pipe_table() {
        let html = "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>";
        assert_eq!(md(html), "| A | B |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn table_cell_keeps_inline_formatting() {
        let html = "<table><tr><th>H</th></tr><tr><td><b>x</b></td></tr></table>";
        assert_eq!(md(html), "| H |\n| --- |\n| **x** |");
    }

    #[test]
    fn link_inside_paragraph() {
        let out = md(r#"<p>see <a href="https://x.y">here</a> now</p>"#);
        assert_eq!(out, "see [here](https://x.y) now");
    }
}
