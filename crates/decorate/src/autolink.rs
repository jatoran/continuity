//! Phase B12 bare-URL auto-link detection.
//!
//! Walks plain text looking for `https://…`, `http://…`, `www.…`,
//! `mailto:…`, and `name@host.tld` patterns and reports them as byte
//! ranges. The renderer treats the reported ranges as link
//! decorations (no source mutation, paint-only — same wire-up as
//! existing `[text](url)` links and CommonMark `<url>` autolinks).
//!
//! Detection is intentionally conservative: only the well-known URL
//! prefixes count, so `foo://bar` and other custom schemes pass
//! through untouched. The set is wide enough for the common case but
//! narrow enough that prose like "see config.toml in §3.2" doesn't
//! trip the detector.

use std::ops::Range;

/// One detected bare URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoLink {
    /// Half-open byte range in the source text.
    pub range: Range<usize>,
    /// The kind of URL — drives the URL the renderer hands the OS
    /// shell when the user Ctrl+clicks.
    pub kind: AutoLinkKind,
}

/// Subset of URL flavours the auto-linker recognises.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AutoLinkKind {
    /// `https://…` or `http://…`.
    Http,
    /// `www.…` — the renderer prefixes `https://` when launching.
    WwwBare,
    /// `name@host.tld`.
    Email,
    /// `mailto:name@host.tld`.
    MailtoExplicit,
}

/// Scan `text` and return the bare-URL byte ranges in source order.
#[must_use]
pub fn auto_links(text: &str) -> Vec<AutoLink> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Token boundary: a URL/email can only start at the document
        // start or after a non-URL-byte (whitespace / punctuation
        // that isn't part of a URL).
        if i > 0 && is_url_inner_byte(bytes[i - 1]) {
            i += 1;
            continue;
        }
        if let Some((kind, len)) = match_prefix(&bytes[i..]) {
            let end = i + scan_url_tail(&bytes[i + len..]) + len;
            let end = trim_trailing_punct(bytes, i, end);
            if end > i + len {
                out.push(AutoLink {
                    range: i..end,
                    kind,
                });
                i = end;
                continue;
            }
        }
        if let Some(range) = match_email(bytes, i) {
            i = range.end;
            out.push(AutoLink {
                range,
                kind: AutoLinkKind::Email,
            });
            continue;
        }
        i += 1;
    }
    out
}

fn match_prefix(bytes: &[u8]) -> Option<(AutoLinkKind, usize)> {
    if bytes.starts_with(b"https://") {
        return Some((AutoLinkKind::Http, 8));
    }
    if bytes.starts_with(b"http://") {
        return Some((AutoLinkKind::Http, 7));
    }
    if bytes.starts_with(b"mailto:") {
        return Some((AutoLinkKind::MailtoExplicit, 7));
    }
    if bytes.starts_with(b"www.") && bytes.len() > 4 {
        return Some((AutoLinkKind::WwwBare, 0));
    }
    None
}

fn scan_url_tail(rest: &[u8]) -> usize {
    let mut n = 0;
    while n < rest.len() && is_url_inner_byte(rest[n]) {
        n += 1;
    }
    n
}

fn is_url_inner_byte(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
        | b'-' | b'.' | b'_' | b'~' | b':' | b'/' | b'?' | b'#'
        | b'[' | b']' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'*'
        | b'+' | b',' | b';' | b'=' | b'%'
    )
}

/// Drop trailing punctuation that's clearly sentence-glue rather
/// than part of the URL. Matches CommonMark autolink trimming
/// closely enough for prose use.
fn trim_trailing_punct(bytes: &[u8], start: usize, mut end: usize) -> usize {
    while end > start
        && matches!(
            bytes[end - 1],
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']'
        )
    {
        end -= 1;
    }
    end
}

fn match_email(bytes: &[u8], i: usize) -> Option<Range<usize>> {
    // Email = atext+ '@' atext+ '.' tld
    let local_start = i;
    let mut p = i;
    while p < bytes.len() && is_email_local_byte(bytes[p]) {
        p += 1;
    }
    if p == local_start || p >= bytes.len() || bytes[p] != b'@' {
        return None;
    }
    let at = p;
    if at > 0 && is_email_local_byte(bytes[at - 1]).then_some(()).is_none() {
        return None;
    }
    p += 1;
    let host_start = p;
    while p < bytes.len() && is_email_host_byte(bytes[p]) {
        p += 1;
    }
    if p == host_start {
        return None;
    }
    let host = &bytes[host_start..p];
    if !host.contains(&b'.') {
        return None;
    }
    let end = trim_trailing_punct(bytes, local_start, p);
    Some(local_start..end)
}

fn is_email_local_byte(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
        | b'.' | b'_' | b'-' | b'+'
    )
}

fn is_email_host_byte(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_https_url() {
        let ls = auto_links("see https://example.com today");
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].kind, AutoLinkKind::Http);
        assert_eq!(
            &"see https://example.com today"[ls[0].range.clone()],
            "https://example.com"
        );
    }

    #[test]
    fn detects_http_url_with_path() {
        let ls = auto_links("go http://a.b/c?d=1#e please");
        assert_eq!(ls.len(), 1);
        assert_eq!(
            &"go http://a.b/c?d=1#e please"[ls[0].range.clone()],
            "http://a.b/c?d=1#e"
        );
    }

    #[test]
    fn detects_bare_www() {
        let ls = auto_links("try www.example.com.");
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].kind, AutoLinkKind::WwwBare);
        // Trailing dot trimmed.
        assert_eq!(
            &"try www.example.com."[ls[0].range.clone()],
            "www.example.com"
        );
    }

    #[test]
    fn detects_simple_email() {
        let ls = auto_links("ping me@host.tld");
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].kind, AutoLinkKind::Email);
        assert_eq!(&"ping me@host.tld"[ls[0].range.clone()], "me@host.tld");
    }

    #[test]
    fn detects_mailto_explicit() {
        let ls = auto_links("contact mailto:a@b.c");
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].kind, AutoLinkKind::MailtoExplicit);
        assert_eq!(&"contact mailto:a@b.c"[ls[0].range.clone()], "mailto:a@b.c");
    }

    #[test]
    fn rejects_non_url_token_with_colon() {
        let ls = auto_links("see config.toml or version:1.2.3 below");
        assert!(ls.is_empty());
    }

    #[test]
    fn trims_sentence_trailing_punctuation() {
        let ls = auto_links("done: https://a.b/c?x).");
        assert_eq!(ls.len(), 1);
        assert_eq!(
            &"done: https://a.b/c?x)."[ls[0].range.clone()],
            "https://a.b/c?x"
        );
    }

    #[test]
    fn does_not_match_inside_existing_url() {
        // The second `https://` after another URL char wouldn't be
        // re-matched because the boundary check fails. Two distinct
        // URLs separated by whitespace are both found.
        let ls = auto_links("a https://x.com b https://y.com");
        assert_eq!(ls.len(), 2);
    }

    #[test]
    fn ignores_bare_dot_strings() {
        let ls = auto_links("file.txt section.heading");
        assert!(ls.is_empty());
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(auto_links("").is_empty());
    }
}
