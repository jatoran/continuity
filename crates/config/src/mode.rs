//! Typed views over string-valued settings: `PersistenceMode`,
//! `RevealMode`, `CaretStyle`, `TabCloseButton`, `ThemeMode`.
//!
//! Values are stored as strings in `settings.toml` (so the file is
//! diff-friendly and editable by hand), but consumers want a typed enum
//! with no chance of a stray string at the call site. Conversion happens
//! in `validate` and emits [`crate::Error::Invalid`] on a bad value.

use crate::Error;

/// `[persistence].mode` — durability profile.
///
/// Maps onto the SQLite `synchronous` PRAGMA per spec §4.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum PersistenceMode {
    /// Default: `synchronous=NORMAL`, snapshot policy as configured.
    #[default]
    Balanced,
    /// `synchronous=FULL` — pay extra fsync to survive OS-level crashes.
    MaxSafety,
    /// `synchronous=OFF` — don't fsync at all. Data loss on crash.
    MaxSpeed,
}

impl PersistenceMode {
    /// Parse the string form used in `settings.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when `s` is not one of the three known
    /// values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "balanced" => Ok(Self::Balanced),
            "max_safety" => Ok(Self::MaxSafety),
            "max_speed" => Ok(Self::MaxSpeed),
            other => Err(Error::invalid_enum(
                "persistence.mode",
                other,
                "balanced | max_safety | max_speed",
            )),
        }
    }

    /// String form for the matching SQLite `synchronous` PRAGMA value.
    #[must_use]
    pub fn synchronous_pragma(self) -> &'static str {
        match self {
            Self::Balanced => "NORMAL",
            Self::MaxSafety => "FULL",
            Self::MaxSpeed => "OFF",
        }
    }

    /// String form used in `settings.toml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::MaxSafety => "max_safety",
            Self::MaxSpeed => "max_speed",
        }
    }
}

/// `[markdown].reveal_mode`.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum RevealMode {
    /// Reveal every marker in the entire block on caret entry.
    #[default]
    Block,
    /// Reveal only the markers on the caret's line.
    Line,
}

impl RevealMode {
    /// Parse the string form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "block" => Ok(Self::Block),
            "line" => Ok(Self::Line),
            other => Err(Error::invalid_enum(
                "markdown.reveal_mode",
                other,
                "block | line",
            )),
        }
    }
}

/// `[markdown].dialect` — Phase F7. `Gfm` (default) enables GFM's
/// strict-superset features (tables, task lists, strikethrough,
/// autolinks) plus continuity extensions (inline color, inline table
/// formulas). `CommonMark` tightens to plain CommonMark — reserved
/// hook for a future opt-in; the renderer treats the two identically
/// until that follow-up lands.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum MarkdownDialect {
    /// GFM-compatible + continuity extensions.
    #[default]
    Gfm,
    /// Strict CommonMark — feature flags disabled.
    CommonMark,
}

impl MarkdownDialect {
    /// Parse the string form.
    ///
    /// # Errors
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "gfm" => Ok(Self::Gfm),
            "commonmark" => Ok(Self::CommonMark),
            other => Err(Error::invalid_enum(
                "markdown.dialect",
                other,
                "gfm | commonmark",
            )),
        }
    }
}

/// `[editor].caret_style`.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum CaretStyle {
    /// Thin vertical bar.
    #[default]
    Bar,
    /// Block covering the grapheme.
    Block,
    /// Underline below the grapheme.
    Underline,
}

impl CaretStyle {
    /// Parse the string form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "bar" => Ok(Self::Bar),
            "block" => Ok(Self::Block),
            "underline" => Ok(Self::Underline),
            other => Err(Error::invalid_enum(
                "editor.caret_style",
                other,
                "bar | block | underline",
            )),
        }
    }
}

/// `[ui].tab_close_button`.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum TabCloseButton {
    /// Always render the close button.
    Always,
    /// Render only on hover.
    #[default]
    Hover,
    /// Never render.
    Never,
}

impl TabCloseButton {
    /// Parse the string form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "always" => Ok(Self::Always),
            "hover" => Ok(Self::Hover),
            "never" => Ok(Self::Never),
            other => Err(Error::invalid_enum(
                "ui.tab_close_button",
                other,
                "always | hover | never",
            )),
        }
    }
}

/// `[ui].theme` — system / dark / light follow-the-OS mode.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ThemeMode {
    /// Follow the OS dark/light setting.
    #[default]
    System,
    /// Force the dark theme.
    Dark,
    /// Force the light theme.
    Light,
}

impl ThemeMode {
    /// Parse the string form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "system" => Ok(Self::System),
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            other => Err(Error::invalid_enum(
                "ui.theme",
                other,
                "system | dark | light",
            )),
        }
    }
}

/// `[focus].initial_mode` — Phase H1 granular focus mode.
///
/// `view.cycle_focus` walks `Off → Line → Sentence → Paragraph → Off`.
/// The cycle order is encoded in [`Self::next`] so consumers can step
/// without re-implementing it.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum FocusMode {
    /// No dim — everything at full contrast.
    #[default]
    Off,
    /// Dim everything outside the caret's source line.
    Line,
    /// Dim everything outside the caret's sentence.
    Sentence,
    /// Dim everything outside the caret's paragraph / markdown block.
    Paragraph,
}

impl FocusMode {
    /// Parse the string form used in `settings.toml` / command args.
    ///
    /// # Errors
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "off" => Ok(Self::Off),
            "line" => Ok(Self::Line),
            "sentence" => Ok(Self::Sentence),
            "paragraph" => Ok(Self::Paragraph),
            other => Err(Error::invalid_enum(
                "focus.initial_mode",
                other,
                "off | line | sentence | paragraph",
            )),
        }
    }

    /// Next mode in the §H1 cycle order (`off → line → sentence →
    /// paragraph → off`).
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Line,
            Self::Line => Self::Sentence,
            Self::Sentence => Self::Paragraph,
            Self::Paragraph => Self::Off,
        }
    }

    /// String form used in `settings.toml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Line => "line",
            Self::Sentence => "sentence",
            Self::Paragraph => "paragraph",
        }
    }
}

/// Phase C1 — one segment of the status bar. The order in
/// `[statusbar].segments` controls left-to-right paint order.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StatusBarSegment {
    /// `Ln L, Col C` for the primary caret.
    Position,
    /// Total character count of the buffer.
    Chars,
    /// Total word count.
    Words,
    /// `non_empty / total` line count.
    Lines,
    /// Selection char / word / line stats — empty when nothing selected.
    Selection,
    /// Live numeric sum of selected numeric tokens.
    NumericSum,
    /// Source-file encoding (UTF-8 / UTF-16-LE / …).
    Encoding,
    /// Source-file line endings (LF / CRLF / CR / Mixed).
    LineEndings,
    /// `plain` | `markdown` language tag.
    Language,
    /// δ.2 — "idle Xm ago" indicator. Suppressed (`None`) while the
    /// editor is actively used; appears once idle time exceeds five
    /// minutes.
    IdleStale,
}

impl StatusBarSegment {
    /// Parse the string form used in `settings.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for unknown values.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "position" => Ok(Self::Position),
            "chars" => Ok(Self::Chars),
            "words" => Ok(Self::Words),
            "lines" => Ok(Self::Lines),
            "selection" => Ok(Self::Selection),
            "numeric_sum" => Ok(Self::NumericSum),
            "encoding" => Ok(Self::Encoding),
            "line_endings" => Ok(Self::LineEndings),
            "language" => Ok(Self::Language),
            "idle_stale" => Ok(Self::IdleStale),
            other => Err(Error::invalid_enum(
                "statusbar.segments",
                other,
                "position | chars | words | lines | selection | numeric_sum | encoding | line_endings | language | idle_stale",
            )),
        }
    }

    /// String form used in `settings.toml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::Chars => "chars",
            Self::Words => "words",
            Self::Lines => "lines",
            Self::Selection => "selection",
            Self::NumericSum => "numeric_sum",
            Self::Encoding => "encoding",
            Self::LineEndings => "line_endings",
            Self::Language => "language",
            Self::IdleStale => "idle_stale",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_mode_round_trip() {
        for s in ["balanced", "max_safety", "max_speed"] {
            assert_eq!(PersistenceMode::parse(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn synchronous_pragma_maps() {
        assert_eq!(PersistenceMode::Balanced.synchronous_pragma(), "NORMAL");
        assert_eq!(PersistenceMode::MaxSafety.synchronous_pragma(), "FULL");
        assert_eq!(PersistenceMode::MaxSpeed.synchronous_pragma(), "OFF");
    }

    #[test]
    fn rejects_unknown_values() {
        assert!(matches!(
            PersistenceMode::parse("safe"),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(
            RevealMode::parse("paragraph"),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(
            CaretStyle::parse("blink"),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(
            TabCloseButton::parse("none"),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(
            ThemeMode::parse("paper"),
            Err(Error::Invalid { .. })
        ));
    }

    #[test]
    fn status_bar_segment_round_trip() {
        for s in [
            "position",
            "chars",
            "words",
            "lines",
            "selection",
            "numeric_sum",
            "encoding",
            "line_endings",
            "language",
            "idle_stale",
        ] {
            assert_eq!(StatusBarSegment::parse(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn status_bar_segment_rejects_unknown() {
        assert!(matches!(
            StatusBarSegment::parse("weather"),
            Err(Error::Invalid { .. })
        ));
    }

    #[test]
    fn focus_mode_round_trip() {
        for s in ["off", "line", "sentence", "paragraph"] {
            assert_eq!(FocusMode::parse(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn focus_mode_cycle_walks_through_each_state() {
        let mut m = FocusMode::Off;
        m = m.next();
        assert_eq!(m, FocusMode::Line);
        m = m.next();
        assert_eq!(m, FocusMode::Sentence);
        m = m.next();
        assert_eq!(m, FocusMode::Paragraph);
        m = m.next();
        assert_eq!(m, FocusMode::Off);
    }

    #[test]
    fn focus_mode_rejects_unknown() {
        assert!(matches!(
            FocusMode::parse("typewriter"),
            Err(Error::Invalid { .. })
        ));
    }
}
