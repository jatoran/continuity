//! Phase B13 smart paste — URL / image only.
//!
//! When the clipboard contents are a single bare URL, `Ctrl+V`
//! transforms the paste into a markdown link / image reference based
//! on the active selection state:
//!
//! * selection non-empty → `[selection](url)`
//! * no selection, image-extension url → `![](url)`
//! * no selection, plain url → `<url>` autolink
//!
//! Anything that isn't a single URL pastes as plain text. The plain
//! fallback (`Ctrl+Shift+V`) bypasses this transform entirely — it
//! routes through `insert_plain_clipboard_text`, which skips both this
//! URL transform and the clipboard-image branch and inserts the raw
//! `CF_UNICODETEXT` payload.
//!
//! The indent-normalisation variant of "smart paste" (original
//! decisions item 27) is **dropped** — see §K in
//! `roadmap_v2.md`.

use continuity_decorate::auto_links;

/// One of the four smart-paste outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SmartPasteOp {
    /// Selection non-empty + clipboard URL: surround with `[…](url)`.
    WrapAsLink {
        /// Markdown surround prefix — always `"["`.
        open: String,
        /// Markdown surround suffix — `"](<url>)"` so the renderer
        /// keeps the URL exactly as supplied.
        close: String,
    },
    /// No selection + image url: insert `![](url)` at each caret.
    InsertImageRef(String),
    /// No selection + plain url: insert `<url>` autolink.
    InsertBareUrl(String),
}

/// Decide what to do with `clipboard_text` given whether the active
/// selection has any extent. Returns `None` when the paste should
/// fall through to ordinary plain-text behaviour.
pub(crate) fn smart_paste_transform(
    clipboard_text: &str,
    has_selection: bool,
) -> Option<SmartPasteOp> {
    let trimmed = clipboard_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let links = auto_links(trimmed);
    // Require a single auto-link that spans the entire trimmed text.
    if links.len() != 1 || links[0].range != (0..trimmed.len()) {
        return None;
    }
    let url = trimmed.to_string();
    if has_selection {
        return Some(SmartPasteOp::WrapAsLink {
            open: "[".to_string(),
            close: format!("]({url})"),
        });
    }
    if is_image_url(&url) {
        return Some(SmartPasteOp::InsertImageRef(format!("![]({url})")));
    }
    Some(SmartPasteOp::InsertBareUrl(format!("<{url}>")))
}

fn is_image_url(url: &str) -> bool {
    let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_selection_around_url() {
        let op = smart_paste_transform("https://a.b/c", true).expect("op");
        assert_eq!(
            op,
            SmartPasteOp::WrapAsLink {
                open: "[".into(),
                close: "](https://a.b/c)".into(),
            }
        );
    }

    #[test]
    fn no_selection_image_url_becomes_image_ref() {
        let op = smart_paste_transform("https://a.b/c.png", false).expect("op");
        assert_eq!(
            op,
            SmartPasteOp::InsertImageRef("![](https://a.b/c.png)".into())
        );
    }

    #[test]
    fn image_extension_recognised_case_insensitive() {
        let op = smart_paste_transform("https://a.b/C.PNG", false).expect("op");
        assert!(matches!(op, SmartPasteOp::InsertImageRef(_)));
    }

    #[test]
    fn image_url_with_query_string_still_image() {
        let op = smart_paste_transform("https://a.b/c.jpg?v=2", false).expect("op");
        assert!(matches!(op, SmartPasteOp::InsertImageRef(_)));
    }

    #[test]
    fn no_selection_plain_url_becomes_autolink() {
        let op = smart_paste_transform("https://a.b/c", false).expect("op");
        assert_eq!(op, SmartPasteOp::InsertBareUrl("<https://a.b/c>".into()));
    }

    #[test]
    fn whitespace_trimmed_before_decision() {
        let op = smart_paste_transform("  https://a.b/c\n", false).expect("op");
        assert_eq!(op, SmartPasteOp::InsertBareUrl("<https://a.b/c>".into()));
    }

    #[test]
    fn non_url_clipboard_falls_through() {
        assert!(smart_paste_transform("hello world", false).is_none());
        assert!(smart_paste_transform("hello world", true).is_none());
    }

    #[test]
    fn multi_url_clipboard_falls_through() {
        // "Paste a paragraph that happens to mention two URLs" must
        // paste literally — only single-URL clipboards transform.
        assert!(smart_paste_transform("see https://a.b and https://c.d", false).is_none());
    }

    #[test]
    fn empty_clipboard_falls_through() {
        assert!(smart_paste_transform("", false).is_none());
        assert!(smart_paste_transform("   ", true).is_none());
    }
}
