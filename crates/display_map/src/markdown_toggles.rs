//! Per-decoration render toggles applied at display-map projection time.
//!
//! These flags gate *projection* and *paint* only — never decoration
//! production. The decorate crate keeps emitting every inline / block
//! span regardless of these toggles, so cursor-skip
//! (`is_emphasis_marker_byte`) and the decorate golden span counts stay
//! valid. When a toggle is OFF the corresponding markup renders as **raw
//! markdown**: its markers stay visible and unstyled (italic off shows a
//! literal `*foo*`; divider off shows a literal `---`). Source bytes are
//! never mutated.
//!
//! ## Thread ownership
//!
//! Plain `Copy` data. Built on the UI thread from
//! `continuity_config::MarkdownConfig` (via the ui projection) and
//! handed to the display-map builder, which runs on the decoration
//! worker. Carrying it by value keeps `display_map` free of any
//! `config` dependency (config is a leaf; display_map sits above it
//! through `buffer`/`decorate`/`text` only).

use std::hash::{Hash, Hasher};

use ahash::AHasher;

/// The five markdown render toggles, defaulting to the shipped policy:
/// italic OFF, everything else ON.
///
/// * `italic` — `*foo*` / `_foo_` emphasis styling + delimiter hide.
/// * `bold` — `**foo**` / `__foo__` strong styling + delimiter hide.
/// * `highlight` — `==text==` delimiter hide (display map) + highlight
///   background paint (render). Independent of `{#hex:}` color, which
///   shares the inline-color pass but is *not* gated here.
/// * `setext_heading` — setext `===` / `---` heading-underline rendering
///   (`BlockKind::SetextHeading`). When OFF the underline row + heading
///   styling are suppressed and the raw text renders unscaled.
/// * `divider` — `---` / `***` / `___` thematic-break marker hide
///   (display map) + horizontal-rule paint (render).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MarkdownRenderToggles {
    /// Render `*foo*` / `_foo_` emphasis (italic). Default OFF.
    pub italic: bool,
    /// Render `**foo**` / `__foo__` strong (bold). Default ON.
    pub bold: bool,
    /// Render `==text==` highlight (background fill). Default ON.
    pub highlight: bool,
    /// Render setext heading underlines (`===` / `---`). Default ON.
    pub setext_heading: bool,
    /// Render `---` / `***` / `___` thematic-break dividers. Default ON.
    pub divider: bool,
}

impl Default for MarkdownRenderToggles {
    fn default() -> Self {
        Self {
            italic: false,
            bold: true,
            highlight: true,
            setext_heading: true,
            divider: true,
        }
    }
}

impl MarkdownRenderToggles {
    /// All five toggles ON. Used by callers that want the pre-toggle
    /// "render everything" behaviour (e.g. raw-render test contexts that
    /// expect emphasis styling without configuring the projection).
    #[must_use]
    pub const fn all_on() -> Self {
        Self {
            italic: true,
            bold: true,
            highlight: true,
            setext_heading: true,
            divider: true,
        }
    }

    /// Stable 64-bit discriminator for this toggle set. Folded into the
    /// font-state / segment-cache keys on the UI side so a hot-reload
    /// toggle flip invalidates every cached frame, segment list, and
    /// wrap profile built against the previous toggles. Two equal toggle
    /// sets always hash equal; differing ones differ with overwhelming
    /// probability.
    #[must_use]
    pub fn hash_key(self) -> u64 {
        let mut hasher = AHasher::default();
        self.hash(&mut hasher);
        hasher.finish()
    }
}
