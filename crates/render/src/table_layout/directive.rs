//! Phase F — pipe-table presentation directive.
//!
//! Column widths and the wrap/clip mode are **encoded in the raw
//! markdown** so the source text stays the source of truth and the
//! visual table renders deterministically. The directive is a single
//! HTML-comment line immediately above the table:
//!
//! ```text
//! <!--continuity:width=120,-,80;wrap=on-->
//! | Name | City | Qty |
//! |------|------|-----|
//! | …    | …    | …   |
//! ```
//!
//! - `width=` — per-column width in DIPs, in source column order. `-`
//!   (or an empty / unparsable entry) means "auto-size this column".
//!   Trailing columns with no entry are auto.
//! - `wrap=` — `on` (cells wrap to extra visual rows) or `off` (cells
//!   stay one line per `<br>` segment and clip at the column edge).
//!
//! Parsing and formatting live here so both the renderer (which applies
//! the directive) and the UI (which rewrites it on a column drag or a
//! wrap toggle) share one canonical encoding.
//!
//! Thread ownership: pure data, callable from any thread.

/// Marker that opens a continuity table directive comment.
pub const TABLE_DIRECTIVE_PREFIX: &str = "<!--continuity:";
const TABLE_DIRECTIVE_SUFFIX: &str = "-->";

/// Parsed presentation directive for one table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableDirective {
    /// Per-column explicit width in DIPs (`None` = auto-size). May be
    /// shorter than the column count; missing trailing columns are auto.
    pub widths: Vec<Option<f32>>,
    /// `true` → cells wrap to additional visual rows; `false` → cells
    /// stay one line per `<br>` segment and clip at the column edge.
    pub wrap: bool,
}

impl Default for TableDirective {
    fn default() -> Self {
        Self {
            widths: Vec::new(),
            wrap: true,
        }
    }
}

/// `true` when `line` (trimmed) is a continuity table directive comment.
#[must_use]
pub fn is_table_directive_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with(TABLE_DIRECTIVE_PREFIX) && trimmed.ends_with(TABLE_DIRECTIVE_SUFFIX)
}

/// Parse a directive line. Returns `None` when `line` is not a
/// continuity directive comment. Unknown keys are ignored so the format
/// can grow without breaking older parsers.
#[must_use]
pub fn parse_table_directive(line: &str) -> Option<TableDirective> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix(TABLE_DIRECTIVE_PREFIX)?
        .strip_suffix(TABLE_DIRECTIVE_SUFFIX)?
        .trim();
    let mut directive = TableDirective::default();
    for part in inner.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("width=") {
            directive.widths = value
                .split(',')
                .map(|entry| {
                    let entry = entry.trim();
                    if entry == "-" || entry.is_empty() {
                        None
                    } else {
                        entry
                            .parse::<f32>()
                            .ok()
                            .filter(|w| w.is_finite() && *w > 0.0)
                    }
                })
                .collect();
        } else if let Some(value) = part.strip_prefix("wrap=") {
            directive.wrap = matches!(value.trim(), "on" | "true" | "1" | "yes");
        }
    }
    Some(directive)
}

/// Format a directive line (no trailing newline). `widths` is per-column
/// in source order; `None` entries serialize as `-`.
#[must_use]
pub fn format_table_directive(widths: &[Option<f32>], wrap: bool) -> String {
    let widths_str = if widths.is_empty() {
        "-".to_string()
    } else {
        widths
            .iter()
            .map(|w| match w {
                Some(v) => (v.round() as i32).max(1).to_string(),
                None => "-".to_string(),
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "{TABLE_DIRECTIVE_PREFIX}width={widths_str};wrap={}{TABLE_DIRECTIVE_SUFFIX}",
        if wrap { "on" } else { "off" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_widths_and_wrap() {
        let d = parse_table_directive("<!--continuity:width=120,-,80;wrap=off-->").unwrap();
        assert_eq!(d.widths, vec![Some(120.0), None, Some(80.0)]);
        assert!(!d.wrap);
    }

    #[test]
    fn wrap_defaults_on_and_is_case_token() {
        let d = parse_table_directive("<!--continuity:width=-->").unwrap();
        assert!(d.wrap);
        let off = parse_table_directive("<!--continuity:wrap=off-->").unwrap();
        assert!(!off.wrap);
    }

    #[test]
    fn non_directive_line_is_none() {
        assert!(parse_table_directive("| a | b |").is_none());
        assert!(parse_table_directive("<!-- ordinary comment -->").is_none());
    }

    #[test]
    fn round_trips() {
        let line = format_table_directive(&[Some(120.0), None, Some(80.0)], false);
        let parsed = parse_table_directive(&line).unwrap();
        assert_eq!(parsed.widths, vec![Some(120.0), None, Some(80.0)]);
        assert!(!parsed.wrap);
    }

    #[test]
    fn is_directive_line_detects_with_surrounding_space() {
        assert!(is_table_directive_line("   <!--continuity:wrap=on-->  "));
        assert!(!is_table_directive_line("text"));
    }
}
