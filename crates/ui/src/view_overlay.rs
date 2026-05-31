//! Phase-A transient view-overlay layer.
//!
//! A per-pane override layer that sits **above** settings (the persisted
//! `settings.toml` values) and **below** persisted state (`view_states`
//! rows). Used for preview-while-hovering without dirtying the settings
//! file or the persistence queue.
//!
//! Consumers:
//! - Font picker preview (`roadmap_v2.md` §E3) — hover a row, the
//!   overlay carries the previewed `font_family` until commit or cancel.
//! - Theme picker preview (§E4) — same shape with `theme_name`.
//! - Timeline scrubber preview (§I1) — overlay carries a pinned buffer
//!   revision so the renderer shows a past snapshot without mutating the
//!   rope.
//!
//! Design rule: every overlay field is `Option<T>` and `None` means
//! "fall through to the settings layer". An overlay value never enters
//! the persistence queue — committing a picker writes through to
//! settings then clears the overlay (see [`ViewOverlay::commit_clear`]).
//!
//! Thread ownership: UI thread of one window. One overlay per pane.

use continuity_buffer::Revision;

/// Single-pane transient overlay. Values that are `Some(_)` win over the
/// settings layer; `None` falls through to settings.
///
/// New previewable fields land here; keep them `Option<T>` and add a
/// helper accessor that consumers can call without knowing about the
/// overlay machinery.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ViewOverlay {
    /// Preview-only prose font family. Settings layer remains unchanged.
    pub font_family: Option<String>,
    /// Preview-only monospace font family.
    pub font_family_mono: Option<String>,
    /// Preview-only theme name (e.g. when hovering rows in the theme
    /// picker).
    pub theme_name: Option<String>,
    /// Pinned buffer revision for the timeline scrubber. When set, the
    /// renderer should display the buffer's content at this revision
    /// instead of the head snapshot. Cleared on commit/cancel.
    pub pinned_revision: Option<Revision>,
}

impl ViewOverlay {
    /// An empty overlay (no overrides active).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when at least one field is set. Used by the renderer as a
    /// fast bail-out: when no overlay is active, skip the per-field
    /// `Option` checks and just use the settings layer directly.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.font_family.is_some()
            || self.font_family_mono.is_some()
            || self.theme_name.is_some()
            || self.pinned_revision.is_some()
    }

    /// Drop every override. Call on `commit` after writing through to
    /// settings, and on `cancel` to revert.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Resolve the effective prose font family for this pane.
    /// Returns the overlay value if set, otherwise `settings_value`.
    ///
    /// Consumers should call this instead of reading the settings field
    /// directly so previews work without an `if overlay.is_active()`
    /// branch at every paint site.
    #[must_use]
    pub fn font_family<'a>(&'a self, settings_value: &'a str) -> &'a str {
        self.font_family.as_deref().unwrap_or(settings_value)
    }

    /// Resolve the effective monospace font family.
    #[must_use]
    pub fn font_family_mono<'a>(&'a self, settings_value: &'a str) -> &'a str {
        self.font_family_mono.as_deref().unwrap_or(settings_value)
    }

    /// Resolve the effective theme name.
    #[must_use]
    pub fn theme_name<'a>(&'a self, settings_value: &'a str) -> &'a str {
        self.theme_name.as_deref().unwrap_or(settings_value)
    }

    /// Resolve the effective buffer revision the renderer should display.
    #[must_use]
    pub fn revision_or(&self, head: Revision) -> Revision {
        self.pinned_revision.unwrap_or(head)
    }

    /// Commit shorthand: the picker has written through to the settings
    /// layer, so the overlay must be released so the renderer falls
    /// through to the newly-persisted value next paint.
    pub fn commit_clear(&mut self) {
        self.clear();
    }

    /// Cancel shorthand: identical to [`Self::clear`] today. Kept as a
    /// named entry point so call sites read self-documenting and we can
    /// hang per-field cancellation policy here later if needed.
    pub fn cancel(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_overlay_is_inactive() {
        let o = ViewOverlay::new();
        assert!(!o.is_active());
    }

    #[test]
    fn font_family_falls_through_when_unset() {
        let o = ViewOverlay::new();
        assert_eq!(o.font_family("Segoe UI Variable"), "Segoe UI Variable");
    }

    #[test]
    fn font_family_wins_when_set() {
        let mut o = ViewOverlay::new();
        o.font_family = Some("Inter".into());
        assert_eq!(o.font_family("Segoe UI Variable"), "Inter");
        assert!(o.is_active());
    }

    #[test]
    fn theme_name_round_trip() {
        let mut o = ViewOverlay::new();
        assert_eq!(o.theme_name("deep_minimal"), "deep_minimal");
        o.theme_name = Some("solarized_dark".into());
        assert_eq!(o.theme_name("deep_minimal"), "solarized_dark");
    }

    #[test]
    fn pinned_revision_or_head() {
        let mut o = ViewOverlay::new();
        let head = Revision(42);
        assert_eq!(o.revision_or(head), head);
        let pinned = Revision(7);
        o.pinned_revision = Some(pinned);
        assert_eq!(o.revision_or(head), pinned);
    }

    #[test]
    fn clear_drops_every_field() {
        let mut o = ViewOverlay {
            font_family: Some("A".into()),
            font_family_mono: Some("B".into()),
            theme_name: Some("C".into()),
            pinned_revision: Some(Revision(1)),
        };
        assert!(o.is_active());
        o.clear();
        assert!(!o.is_active());
        assert_eq!(o, ViewOverlay::default());
    }

    #[test]
    fn commit_clear_and_cancel_are_identical_today() {
        let mut a = ViewOverlay {
            theme_name: Some("x".into()),
            ..ViewOverlay::default()
        };
        let mut b = a.clone();
        a.commit_clear();
        b.cancel();
        assert_eq!(a, b);
        assert!(!a.is_active());
    }

    #[test]
    fn is_active_tracks_each_field_independently() {
        let mut o = ViewOverlay::new();
        assert!(!o.is_active());
        o.font_family_mono = Some("Cascadia Mono".into());
        assert!(o.is_active());
        o.font_family_mono = None;
        assert!(!o.is_active());
        o.pinned_revision = Some(Revision(3));
        assert!(o.is_active());
    }
}
