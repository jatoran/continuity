//! Hand-rolled, dependency-free HTML tokenizer + DOM builder used by the
//! HTML-to-markdown paste converter ([`crate::html_to_markdown`]).
//!
//! Pasted clipboard HTML is small (a selection, not a whole page), so a
//! straightforward single-pass tokenizer feeding a stack-based tree
//! builder is both fast enough and easy to audit. We deliberately do NOT
//! pull in a real HTML parser — the converter must stay crate-free
//! (project rule: no new dependencies for paste richness).
//!
//! Scope: enough grammar to recover rich-text clipboard structure: tags,
//! quoted attributes, text, comments, declarations, raw-text skipping, void
//! elements, and forgiving recovery. It is not a conformant parser.
//!
//! Thread ownership: pure functions; no shared state. Called on the UI
//! thread from the paste path.

use std::collections::BTreeMap;

mod table;

pub(crate) use table::render_table;

/// A parsed HTML node: either a run of decoded text or an element with
/// attributes and children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HtmlNode {
    /// Decoded character data (entities already resolved).
    Text(String),
    /// An element: lowercased tag name, attribute map, child nodes.
    Element {
        /// Lowercased tag name (e.g. `"a"`, `"strong"`).
        tag: String,
        /// Attributes keyed by lowercased name. Value entities decoded.
        attrs: BTreeMap<String, String>,
        /// Child nodes in document order.
        children: Vec<HtmlNode>,
    },
}

impl HtmlNode {
    /// Convenience accessor for an element attribute.
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        match self {
            HtmlNode::Element { attrs, .. } => attrs.get(name).map(String::as_str),
            HtmlNode::Text(_) => None,
        }
    }
}

/// Void (self-closing) HTML elements that never have children.
fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// One lexical token produced by [`tokenize`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `<tag attr=…>` — name lowercased, `self_closing` set for `<.../>`.
    Start {
        tag: String,
        attrs: BTreeMap<String, String>,
        self_closing: bool,
    },
    /// `</tag>` — name lowercased.
    End(String),
    /// Decoded text run.
    Text(String),
}

/// Parse `html` into a forest of [`HtmlNode`]s.
///
/// Forgiving by design: stray end tags are ignored, unclosed elements are
/// auto-closed at their parent's end, and `<script>` / `<style>` bodies
/// are discarded.
pub(crate) fn parse_html(html: &str) -> Vec<HtmlNode> {
    let tokens = tokenize(html);
    build_tree(tokens)
}

/// Lex `html` into a flat token stream.
fn tokenize(html: &str) -> Vec<Token> {
    let bytes = html.as_bytes();
    let mut tokens = Vec::new();
    let mut idx = 0usize;
    let len = bytes.len();
    let mut text_start = 0usize;

    let flush_text = |tokens: &mut Vec<Token>, slice: &str| {
        if slice.is_empty() {
            return;
        }
        let decoded = decode_entities(slice);
        tokens.push(Token::Text(decoded));
    };

    while idx < len {
        if bytes[idx] != b'<' {
            idx += 1;
            continue;
        }
        // Emit any pending text before this `<`.
        flush_text(&mut tokens, &html[text_start..idx]);

        // Comment / doctype / declaration: `<!-- ... -->` or `<!...>`.
        if html[idx..].starts_with("<!--") {
            if let Some(rel_end) = html[idx + 4..].find("-->") {
                idx = idx + 4 + rel_end + 3;
            } else {
                idx = len;
            }
            text_start = idx;
            continue;
        }
        if idx + 1 < len && bytes[idx + 1] == b'!' {
            // Doctype or other declaration — skip to `>`.
            if let Some(rel_end) = html[idx..].find('>') {
                idx += rel_end + 1;
            } else {
                idx = len;
            }
            text_start = idx;
            continue;
        }

        // End tag `</name>`.
        if idx + 1 < len && bytes[idx + 1] == b'/' {
            if let Some(rel_end) = html[idx..].find('>') {
                let inner = &html[idx + 2..idx + rel_end];
                let name = inner.trim().to_ascii_lowercase();
                if !name.is_empty() {
                    tokens.push(Token::End(name));
                }
                idx += rel_end + 1;
            } else {
                idx = len;
            }
            text_start = idx;
            continue;
        }

        // Start tag `<name ...>` — find the matching `>` that is not
        // inside a quoted attribute value.
        match find_tag_end(html, idx) {
            Some(tag_end) => {
                let inner = &html[idx + 1..tag_end];
                if let Some(token) = parse_start_tag(inner) {
                    // Raw-text elements: skip their body wholesale.
                    if let Token::Start {
                        tag, self_closing, ..
                    } = &token
                    {
                        let is_raw = matches!(tag.as_str(), "script" | "style");
                        let closes_immediately = *self_closing;
                        let tag_name = tag.clone();
                        tokens.push(token);
                        idx = tag_end + 1;
                        if is_raw && !closes_immediately {
                            idx = skip_raw_text(html, idx, &tag_name);
                            // Push a synthetic end so the tree balances.
                            tokens.push(Token::End(tag_name));
                        }
                        text_start = idx;
                        continue;
                    }
                    tokens.push(token);
                }
                idx = tag_end + 1;
                text_start = idx;
            }
            None => {
                // No closing `>`; treat the rest as text.
                idx = len;
                flush_text(&mut tokens, &html[text_start..idx]);
                text_start = idx;
            }
        }
    }
    flush_text(&mut tokens, &html[text_start..len]);
    tokens
}

/// Find the index of the `>` that terminates the start tag beginning at
/// `open` (the `<`), skipping any `>` inside single- or double-quoted
/// attribute values.
fn find_tag_end(html: &str, open: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut idx = open + 1;
    let mut quote: Option<u8> = None;
    while idx < bytes.len() {
        let byte = bytes[idx];
        match quote {
            Some(q) => {
                if byte == q {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => return Some(idx),
                _ => {}
            },
        }
        idx += 1;
    }
    None
}

/// Parse the inside of a start tag (between `<` and `>`), returning a
/// [`Token::Start`]. `inner` excludes the angle brackets.
fn parse_start_tag(inner: &str) -> Option<Token> {
    let mut inner = inner.trim();
    let self_closing = inner.ends_with('/');
    if self_closing {
        inner = inner[..inner.len() - 1].trim_end();
    }
    // Tag name = leading run of name characters.
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let tag = inner[..name_end].to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }
    let attrs = parse_attributes(&inner[name_end..]);
    let void = is_void_element(&tag);
    Some(Token::Start {
        tag,
        attrs,
        self_closing: self_closing || void,
    })
}

/// Parse `name="value"` / `name='value'` / `name=value` / bare `name`
/// attribute pairs from the remainder of a start tag.
fn parse_attributes(mut rest: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(rest.len());
        if name_end == 0 {
            // A stray `=` or similar; skip one char to make progress.
            rest = &rest[1..];
            continue;
        }
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        if let Some(after_eq) = rest.strip_prefix('=') {
            let after_eq = after_eq.trim_start();
            let (value, remainder) = read_attr_value(after_eq);
            attrs.insert(name, decode_entities(value));
            rest = remainder;
        } else {
            // Boolean attribute (no value).
            attrs.insert(name, String::new());
        }
    }
    attrs
}

/// Read a (possibly quoted) attribute value, returning the value and the
/// unconsumed remainder of the tag body.
fn read_attr_value(input: &str) -> (&str, &str) {
    let bytes = input.as_bytes();
    if let Some(&q) = bytes.first() {
        if q == b'"' || q == b'\'' {
            if let Some(rel_close) = input[1..].find(q as char) {
                let value = &input[1..1 + rel_close];
                let remainder = &input[1 + rel_close + 1..];
                return (value, remainder);
            }
            // Unterminated quote — take the rest.
            return (&input[1..], "");
        }
    }
    // Unquoted value: up to next whitespace.
    let end = input
        .find(|c: char| c.is_whitespace())
        .unwrap_or(input.len());
    (&input[..end], &input[end..])
}

/// Skip the raw-text body of `<script>`/`<style>` up to its matching end
/// tag, returning the index just past `</tag>`.
fn skip_raw_text(html: &str, start: usize, tag: &str) -> usize {
    let close = format!("</{tag}");
    let lower = html.to_ascii_lowercase();
    match lower[start..].find(&close) {
        Some(rel) => {
            let after = start + rel;
            // Skip to the `>` that ends the end tag.
            match html[after..].find('>') {
                Some(rel_gt) => after + rel_gt + 1,
                None => html.len(),
            }
        }
        None => html.len(),
    }
}

/// Build a node tree from a flat token stream with forgiving recovery.
fn build_tree(tokens: Vec<Token>) -> Vec<HtmlNode> {
    // Each stack frame: (tag, attrs, children-so-far).
    let mut roots: Vec<HtmlNode> = Vec::new();
    let mut stack: Vec<(String, BTreeMap<String, String>, Vec<HtmlNode>)> = Vec::new();

    let push_node = |stack: &mut Vec<(String, BTreeMap<String, String>, Vec<HtmlNode>)>,
                     roots: &mut Vec<HtmlNode>,
                     node: HtmlNode| {
        match stack.last_mut() {
            Some((_, _, children)) => children.push(node),
            None => roots.push(node),
        }
    };

    for token in tokens {
        match token {
            Token::Text(text) => {
                push_node(&mut stack, &mut roots, HtmlNode::Text(text));
            }
            Token::Start {
                tag,
                attrs,
                self_closing,
            } => {
                if self_closing {
                    push_node(
                        &mut stack,
                        &mut roots,
                        HtmlNode::Element {
                            tag,
                            attrs,
                            children: Vec::new(),
                        },
                    );
                } else {
                    stack.push((tag, attrs, Vec::new()));
                }
            }
            Token::End(tag) => {
                // Close up to and including the nearest matching open
                // element; ignore the end tag entirely if none matches.
                if let Some(match_idx) = stack.iter().rposition(|(t, _, _)| *t == tag) {
                    while stack.len() > match_idx {
                        let (t, a, c) = stack.pop().expect("stack non-empty above match_idx");
                        let node = HtmlNode::Element {
                            tag: t,
                            attrs: a,
                            children: c,
                        };
                        push_node(&mut stack, &mut roots, node);
                    }
                }
            }
        }
    }
    // Auto-close anything left open.
    while let Some((t, a, c)) = stack.pop() {
        let node = HtmlNode::Element {
            tag: t,
            attrs: a,
            children: c,
        };
        push_node(&mut stack, &mut roots, node);
    }
    roots
}

/// Decode the common HTML named + numeric character references in `text`.
///
/// Covers the entities a rich-text clipboard realistically emits; unknown
/// references are left verbatim (so `&unknown;` survives rather than
/// vanishing). Numeric references (`&#160;`, `&#xA0;`) are decoded to
/// their Unicode scalar value when valid.
pub(crate) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'&' {
            // Copy the next full UTF-8 char.
            let ch_len = utf8_char_len(bytes[idx]);
            let end = (idx + ch_len).min(bytes.len());
            out.push_str(&text[idx..end]);
            idx = end;
            continue;
        }
        // Find the terminating `;` within a small window.
        let window_end = (idx + 12).min(bytes.len());
        if let Some(rel_semi) = text[idx..window_end].find(';') {
            let entity = &text[idx + 1..idx + rel_semi];
            if let Some(decoded) = decode_one_entity(entity) {
                out.push_str(&decoded);
                idx += rel_semi + 1;
                continue;
            }
        }
        // Not a recognized entity — emit the literal `&`.
        out.push('&');
        idx += 1;
    }
    out
}

/// Decode a single entity body (the part between `&` and `;`). Returns
/// `None` for unrecognized references.
fn decode_one_entity(entity: &str) -> Option<String> {
    if let Some(rest) = entity.strip_prefix('#') {
        let code = if let Some(hex) = rest.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            rest.parse::<u32>().ok()?
        };
        return char::from_u32(code).map(|c| c.to_string());
    }
    let mapped = match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{00A0}",
        "copy" => "\u{00A9}",
        "reg" => "\u{00AE}",
        "trade" => "\u{2122}",
        "hellip" => "\u{2026}",
        "mdash" => "\u{2014}",
        "ndash" => "\u{2013}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201C}",
        "rdquo" => "\u{201D}",
        "middot" => "\u{00B7}",
        "bull" => "\u{2022}",
        "deg" => "\u{00B0}",
        "times" => "\u{00D7}",
        "divide" => "\u{00F7}",
        "laquo" => "\u{00AB}",
        "raquo" => "\u{00BB}",
        "euro" => "\u{20AC}",
        "pound" => "\u{00A3}",
        "cent" => "\u{00A2}",
        "sect" => "\u{00A7}",
        "para" => "\u{00B6}",
        "dagger" => "\u{2020}",
        "Dagger" => "\u{2021}",
        _ => return None,
    };
    Some(mapped.to_string())
}

/// Byte length of the UTF-8 char whose first byte is `first`.
fn utf8_char_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else if first >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_named_and_numeric_entities() {
        assert_eq!(decode_entities("a &amp; b"), "a & b");
        assert_eq!(decode_entities("x &lt;y&gt; z"), "x <y> z");
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        assert_eq!(decode_entities("a&nbsp;b"), "a\u{00A0}b");
    }

    #[test]
    fn unknown_entity_kept_verbatim() {
        assert_eq!(decode_entities("a &bogus; b"), "a &bogus; b");
        assert_eq!(decode_entities("plain & text"), "plain & text");
    }

    #[test]
    fn parses_nested_elements() {
        let nodes = parse_html("<p>Hello <b>bold</b> world</p>");
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            HtmlNode::Element { tag, children, .. } => {
                assert_eq!(tag, "p");
                assert_eq!(children.len(), 3);
                match &children[1] {
                    HtmlNode::Element { tag, .. } => assert_eq!(tag, "b"),
                    other => panic!("expected <b>, got {other:?}"),
                }
            }
            other => panic!("expected <p>, got {other:?}"),
        }
    }

    #[test]
    fn parses_attributes_quoted_and_unquoted() {
        let nodes = parse_html(r#"<a href="https://x.y" title='hi'>link</a>"#);
        let HtmlNode::Element { attrs, .. } = &nodes[0] else {
            panic!("expected element");
        };
        assert_eq!(attrs.get("href").map(String::as_str), Some("https://x.y"));
        assert_eq!(attrs.get("title").map(String::as_str), Some("hi"));
    }

    #[test]
    fn void_element_has_no_children() {
        let nodes = parse_html("<p>a<br>b</p>");
        let HtmlNode::Element { children, .. } = &nodes[0] else {
            panic!("expected <p>");
        };
        // a, <br/>, b
        assert_eq!(children.len(), 3);
        assert!(
            matches!(&children[1], HtmlNode::Element { tag, children, .. } if tag == "br" && children.is_empty())
        );
    }

    #[test]
    fn skips_script_and_style_bodies() {
        let nodes =
            parse_html("<p>keep</p><script>var x = 1 < 2;</script><style>p{}</style><p>also</p>");
        let texts: Vec<String> = collect_text(&nodes);
        let joined = texts.join("");
        assert!(joined.contains("keep"));
        assert!(joined.contains("also"));
        assert!(!joined.contains("var x"));
        assert!(!joined.contains("p{}"));
    }

    #[test]
    fn tolerates_unbalanced_tags() {
        // Missing </b> should auto-close at end of <p>.
        let nodes = parse_html("<p>a<b>bold</p>");
        assert_eq!(nodes.len(), 1);
        let HtmlNode::Element { tag, .. } = &nodes[0] else {
            panic!("expected element");
        };
        assert_eq!(tag, "p");
    }

    #[test]
    fn stray_end_tag_ignored() {
        let nodes = parse_html("hello</b> world");
        let joined = collect_text(&nodes).join("");
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn angle_bracket_inside_attribute_value() {
        let nodes = parse_html(r#"<img alt="a>b" src="u">after"#);
        let HtmlNode::Element { tag, attrs, .. } = &nodes[0] else {
            panic!("expected img");
        };
        assert_eq!(tag, "img");
        assert_eq!(attrs.get("alt").map(String::as_str), Some("a>b"));
        assert_eq!(attrs.get("src").map(String::as_str), Some("u"));
    }

    /// Recursively gather all text node contents (test helper).
    fn collect_text(nodes: &[HtmlNode]) -> Vec<String> {
        let mut out = Vec::new();
        for node in nodes {
            match node {
                HtmlNode::Text(t) => out.push(t.clone()),
                HtmlNode::Element { children, .. } => out.extend(collect_text(children)),
            }
        }
        out
    }
}
