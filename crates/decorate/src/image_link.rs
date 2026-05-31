//! Phase F5 — image link attribute parsing.
//!
//! continuity extends the standard markdown image syntax
//! `![alt](url)` with an optional width hint inside the `alt` slot:
//! `![alt|<width>](url)`. The renderer reads the parsed
//! [`ImageLinkAttrs`] to decide whether to lay the image out at native
//! width (no hint) or scale it to the supplied DIP value.
//!
//! Source bytes stay plain markdown — the `alt|<width>` form is a
//! superset that other markdown tools render with the `|<width>` text
//! intact in the alt attribute, which is acceptable for plain-text
//! interop. Without a hint, the alt parses as-is.
//!
//! Thread ownership: pure, callable from any thread.

/// Result of parsing an `alt[|width]` slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLinkAttrs {
    /// The user-visible alt text, with the `|width` suffix stripped if
    /// one was present.
    pub alt: String,
    /// Explicit width hint in DIPs. `None` ⇒ render at native width
    /// (with the F5 default-on `fit pane width` clamp applied by the
    /// painter).
    pub width: Option<u32>,
}

/// Parse the `alt` slot of an `![alt](url)` markdown image. Returns
/// the alt text plus an optional width hint.
///
/// The width form is `alt|<digits>` with optional whitespace either
/// side of the pipe. A trailing pipe with no digits (or a non-numeric
/// suffix) yields the literal alt verbatim with no width hint.
#[must_use]
pub fn parse_image_alt(raw: &str) -> ImageLinkAttrs {
    let Some(pipe) = raw.rfind('|') else {
        return ImageLinkAttrs {
            alt: raw.to_string(),
            width: None,
        };
    };
    let (alt_part, width_part) = raw.split_at(pipe);
    let width_str = width_part.trim_start_matches('|').trim();
    if width_str.is_empty() {
        return ImageLinkAttrs {
            alt: raw.to_string(),
            width: None,
        };
    }
    match width_str.parse::<u32>() {
        Ok(w) if w > 0 => ImageLinkAttrs {
            alt: alt_part.trim_end().to_string(),
            width: Some(w),
        },
        _ => ImageLinkAttrs {
            alt: raw.to_string(),
            width: None,
        },
    }
}

/// Predicate: is `url` a `images/<hash>.<ext>` reference into the
/// shared image directory (per spec-delta §L#16)? Used by the renderer
/// to decide whether to resolve relative to the shared store.
#[must_use]
pub fn is_shared_store_reference(url: &str) -> bool {
    // Accept forward-slash or backslash separators (Windows paths
    // round-trip through markdown sources with `/` even when written
    // by Win32 code). The marker is the `images/` (or `images\`)
    // prefix.
    let normalized = url.replace('\\', "/");
    normalized.starts_with("images/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_alt_has_no_width() {
        let attrs = parse_image_alt("a screenshot");
        assert_eq!(attrs.alt, "a screenshot");
        assert!(attrs.width.is_none());
    }

    #[test]
    fn alt_with_width_separates() {
        let attrs = parse_image_alt("logo|320");
        assert_eq!(attrs.alt, "logo");
        assert_eq!(attrs.width, Some(320));
    }

    #[test]
    fn whitespace_around_pipe_is_trimmed() {
        let attrs = parse_image_alt("logo | 200");
        assert_eq!(attrs.alt, "logo");
        assert_eq!(attrs.width, Some(200));
    }

    #[test]
    fn trailing_empty_pipe_yields_no_width() {
        let attrs = parse_image_alt("logo|");
        assert_eq!(attrs.alt, "logo|");
        assert!(attrs.width.is_none());
    }

    #[test]
    fn non_numeric_suffix_yields_no_width() {
        let attrs = parse_image_alt("logo|wide");
        assert_eq!(attrs.alt, "logo|wide");
        assert!(attrs.width.is_none());
    }

    #[test]
    fn zero_width_yields_no_width() {
        let attrs = parse_image_alt("x|0");
        assert_eq!(attrs.alt, "x|0");
        assert!(attrs.width.is_none());
    }

    #[test]
    fn rightmost_pipe_wins_when_multiple_pipes() {
        let attrs = parse_image_alt("a|b|320");
        assert_eq!(attrs.alt, "a|b");
        assert_eq!(attrs.width, Some(320));
    }

    #[test]
    fn shared_store_predicate_accepts_forward_and_back_slash() {
        assert!(is_shared_store_reference("images/abc.png"));
        assert!(is_shared_store_reference("images\\abc.png"));
        assert!(!is_shared_store_reference("./images/abc.png"));
        assert!(!is_shared_store_reference("other/abc.png"));
    }
}
