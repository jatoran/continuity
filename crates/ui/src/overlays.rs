//! Overlay state machine: which overlay (if any) is active, and the typed
//! state for each kind.
//!
//! **Thread ownership**: per-window, mutated only on the UI thread that owns
//! the [`crate::Window`].

use crate::find_bar::FindBar;
use crate::find_in_all::FindInAll;
use crate::font_picker::FontPicker;
use crate::goto_overlay::{GotoHeading, GotoLine};
use crate::hex_picker::HexPicker;
use crate::palette::Palette;
use crate::pane_tree::TabId;
use crate::previous_buffer_browser::{PreviousBufferBrowser, PreviousBufferRow};
use crate::quick_open::QuickOpen;
use crate::slash_palette::{SlashPalette, SlashPaletteEntry, SlashTrigger};
use crate::tab_switcher::{TabSwitcher, TabSwitcherRow};
use crate::theme_picker::ThemePicker;

/// Which overlay (if any) currently has input focus.
#[derive(Default)]
pub enum Overlays {
    /// No overlay; keystrokes go to the editor.
    #[default]
    Idle,
    /// In-buffer find / replace bar.
    Find(FindBar),
    /// Find-in-all-buffers panel.
    FindInAll(FindInAll),
    /// Command palette.
    Palette(Palette),
    /// Quick-open buffer switcher.
    QuickOpen(QuickOpen),
    /// Goto-line dialog.
    GotoLine(GotoLine),
    /// Goto-heading picker.
    GotoHeading(GotoHeading),
    /// §E3 — font-picker palette mode with live preview.
    FontPicker(FontPicker),
    /// §E4 — theme-picker palette mode with live preview.
    ThemePicker(ThemePicker),
    /// §H6 — Ctrl+Tab positional tab switcher palette mode.
    TabSwitcher(TabSwitcher),
    /// §H5 — slash-command palette (insertion-only safelist).
    SlashPalette(SlashPalette),
    /// Phase F3 — hex-input picker. Accepts only `[0-9a-fA-F]`; Enter
    /// commits when the digit count is 3 / 4 / 6 / 8.
    HexPicker(HexPicker),
    /// δ.4 — previous-buffer browser palette mode.
    PreviousBufferBrowser(PreviousBufferBrowser),
}

impl Overlays {
    /// Idle (no overlay) state.
    #[must_use]
    pub fn idle() -> Self {
        Self::Idle
    }

    /// `true` when an overlay is active and consuming input.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Discriminant used by the renderer / tests.
    #[must_use]
    pub fn kind(&self) -> OverlayKind {
        match self {
            Self::Idle => OverlayKind::None,
            Self::Find(_) => OverlayKind::Find,
            Self::FindInAll(_) => OverlayKind::FindInAll,
            Self::Palette(_) => OverlayKind::Palette,
            Self::QuickOpen(_) => OverlayKind::QuickOpen,
            Self::GotoLine(_) => OverlayKind::GotoLine,
            Self::GotoHeading(_) => OverlayKind::GotoHeading,
            Self::FontPicker(_) => OverlayKind::FontPicker,
            Self::ThemePicker(_) => OverlayKind::ThemePicker,
            Self::TabSwitcher(_) => OverlayKind::TabSwitcher,
            Self::SlashPalette(_) => OverlayKind::SlashPalette,
            Self::HexPicker(_) => OverlayKind::HexPicker,
            Self::PreviousBufferBrowser(_) => OverlayKind::PreviousBufferBrowser,
        }
    }

    /// Replace the current state with a fresh `kind` overlay.
    ///
    /// **Note**: `OverlayKind::FontPicker`, `OverlayKind::ThemePicker`,
    /// and `OverlayKind::TabSwitcher` cannot be opened through this
    /// path — they each need data captured at open time (font family
    /// list, theme entries, positional tab snapshot), so they are
    /// opened directly via `Overlays::open_font_picker` /
    /// `open_theme_picker` / `open_tab_switcher`.
    pub fn open(&mut self, kind: OverlayKind) {
        *self = match kind {
            OverlayKind::None => Self::Idle,
            OverlayKind::Find => Self::Find(FindBar::new()),
            OverlayKind::FindInAll => Self::FindInAll(FindInAll::new()),
            OverlayKind::Palette => Self::Palette(Palette::new()),
            OverlayKind::QuickOpen => Self::QuickOpen(QuickOpen::new()),
            OverlayKind::GotoLine => Self::GotoLine(GotoLine::new()),
            OverlayKind::GotoHeading => Self::GotoHeading(GotoHeading::new()),
            OverlayKind::FontPicker => return,
            OverlayKind::ThemePicker => return,
            OverlayKind::TabSwitcher => return,
            OverlayKind::SlashPalette => return,
            OverlayKind::HexPicker => Self::HexPicker(HexPicker::new(None)),
            OverlayKind::PreviousBufferBrowser => return,
        };
    }

    /// G2: open the find bar populated from a pre-built [`FindBar`] (typically
    /// restored from a [`crate::find_bar::FindBarMemento`]). Distinct from
    /// `open(OverlayKind::Find)` which builds a fresh empty bar.
    pub(crate) fn open_find_with(&mut self, fb: FindBar) {
        *self = Self::Find(fb);
    }

    /// Open the font picker with the enumerated family list and the
    /// currently-active family name.
    pub(crate) fn open_font_picker(&mut self, families: Vec<String>, original: String) {
        *self = Self::FontPicker(FontPicker::new(families, original));
    }

    /// Open the theme picker with the enumerated theme list, the
    /// `ThemeSet` to revert to on Esc, and the currently-active theme
    /// name (used to anchor the initial highlight).
    pub(crate) fn open_theme_picker(
        &mut self,
        entries: Vec<crate::theme_picker::ThemeEntry>,
        original_set: continuity_theme::ThemeSet,
        original_name: String,
    ) {
        *self = Self::ThemePicker(ThemePicker::new(entries, original_set, original_name));
    }

    /// §H6 — open the tab switcher with the positional `rows` snapshot
    /// and the id of the tab that was active when the chord fired.
    /// `initial_delta` is +1 for `Ctrl+Tab` (cursor pre-advances to the
    /// next tab) or -1 for `Ctrl+Shift+Tab` (cursor pre-advances to the
    /// previous one), matching how every other "tap" of the chord
    /// would have stepped before the overlay appeared.
    pub(crate) fn open_tab_switcher(
        &mut self,
        rows: Vec<TabSwitcherRow>,
        original_active: TabId,
        initial_delta: i32,
    ) {
        *self = Self::TabSwitcher(TabSwitcher::new(rows, original_active, initial_delta));
    }

    /// §H5 — open the slash-command palette anchored at `anchor_line`
    /// with the `palette_safe` insertion safelist captured at open
    /// time and `trigger` recording how the palette opened (so Esc
    /// can decide whether to clean up a trailing `/`).
    pub(crate) fn open_slash_palette(
        &mut self,
        entries: Vec<SlashPaletteEntry>,
        anchor_line: u32,
        trigger: SlashTrigger,
    ) {
        *self = Self::SlashPalette(SlashPalette::new(entries, anchor_line, trigger));
    }

    /// δ.4 — open the previous-buffer browser with `rows` (already
    /// sorted by `last_touched DESC`) and the filter discriminant the
    /// rows were queried under.
    pub(crate) fn open_previous_buffer_browser(
        &mut self,
        rows: Vec<PreviousBufferRow>,
        filter: continuity_persist::BufferListFilter,
    ) {
        let mut browser = PreviousBufferBrowser::new();
        browser.set_filter(filter);
        browser.set_candidates(rows);
        *self = Self::PreviousBufferBrowser(browser);
    }

    /// δ.4 — mutably borrow the active previous-buffer browser, if any.
    pub(crate) fn previous_buffer_browser_mut(&mut self) -> Option<&mut PreviousBufferBrowser> {
        if let Self::PreviousBufferBrowser(b) = self {
            Some(b)
        } else {
            None
        }
    }

    /// δ.4 — borrow the active previous-buffer browser, if any.
    #[must_use]
    pub fn previous_buffer_browser(&self) -> Option<&PreviousBufferBrowser> {
        if let Self::PreviousBufferBrowser(b) = self {
            Some(b)
        } else {
            None
        }
    }

    /// Dismiss any active overlay.
    pub fn dismiss(&mut self) {
        *self = Self::Idle;
    }

    /// Phase F3 — replace the current state with a hex picker seeded by
    /// `prefill`. Use `None` for a blank picker.
    pub fn open_hex_picker(&mut self, prefill: Option<&str>) {
        *self = Self::HexPicker(HexPicker::new(prefill));
    }

    /// Phase F3 — borrow the active hex picker, if any.
    pub fn hex_picker(&self) -> Option<&HexPicker> {
        if let Self::HexPicker(h) = self {
            Some(h)
        } else {
            None
        }
    }

    /// Borrow the active find-bar state, if any.
    pub fn find_bar(&self) -> Option<&FindBar> {
        if let Self::Find(f) = self {
            Some(f)
        } else {
            None
        }
    }

    /// Mutably borrow the active find-bar state, if any.
    pub(crate) fn find_bar_mut(&mut self) -> Option<&mut FindBar> {
        if let Self::Find(f) = self {
            Some(f)
        } else {
            None
        }
    }

    /// Mutably borrow the active palette, if any.
    pub(crate) fn palette_mut(&mut self) -> Option<&mut Palette> {
        if let Self::Palette(p) = self {
            Some(p)
        } else {
            None
        }
    }

    /// Mutably borrow the active quick-open, if any.
    pub(crate) fn quick_open_mut(&mut self) -> Option<&mut QuickOpen> {
        if let Self::QuickOpen(q) = self {
            Some(q)
        } else {
            None
        }
    }

    /// Mutably borrow the active goto-line, if any.
    pub(crate) fn goto_line_mut(&mut self) -> Option<&mut GotoLine> {
        if let Self::GotoLine(g) = self {
            Some(g)
        } else {
            None
        }
    }

    /// Mutably borrow the active goto-heading, if any.
    pub(crate) fn goto_heading_mut(&mut self) -> Option<&mut GotoHeading> {
        if let Self::GotoHeading(g) = self {
            Some(g)
        } else {
            None
        }
    }

    /// Mutably borrow the active find-in-all, if any.
    pub(crate) fn find_in_all_mut(&mut self) -> Option<&mut FindInAll> {
        if let Self::FindInAll(f) = self {
            Some(f)
        } else {
            None
        }
    }

    /// Mutably borrow the active font picker, if any.
    pub fn font_picker_mut(&mut self) -> Option<&mut FontPicker> {
        if let Self::FontPicker(fp) = self {
            Some(fp)
        } else {
            None
        }
    }

    /// Borrow the active font picker, if any.
    #[must_use]
    pub(crate) fn font_picker(&self) -> Option<&FontPicker> {
        if let Self::FontPicker(fp) = self {
            Some(fp)
        } else {
            None
        }
    }

    /// Mutably borrow the active theme picker, if any.
    pub fn theme_picker_mut(&mut self) -> Option<&mut ThemePicker> {
        if let Self::ThemePicker(tp) = self {
            Some(tp)
        } else {
            None
        }
    }

    /// Borrow the active theme picker, if any.
    #[must_use]
    pub(crate) fn theme_picker(&self) -> Option<&ThemePicker> {
        if let Self::ThemePicker(tp) = self {
            Some(tp)
        } else {
            None
        }
    }

    /// Mutably borrow the active tab switcher, if any.
    pub(crate) fn tab_switcher_mut(&mut self) -> Option<&mut TabSwitcher> {
        if let Self::TabSwitcher(ts) = self {
            Some(ts)
        } else {
            None
        }
    }

    /// Borrow the active tab switcher, if any.
    #[must_use]
    pub(crate) fn tab_switcher(&self) -> Option<&TabSwitcher> {
        if let Self::TabSwitcher(ts) = self {
            Some(ts)
        } else {
            None
        }
    }

    /// Borrow the active slash palette, if any.
    #[must_use]
    pub(crate) fn slash_palette(&self) -> Option<&SlashPalette> {
        if let Self::SlashPalette(sp) = self {
            Some(sp)
        } else {
            None
        }
    }
}

/// Overlay discriminant used by commands and tests to request a specific
/// overlay without naming the typed state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    /// No overlay.
    None,
    /// In-buffer find/replace bar.
    Find,
    /// Find-in-all-buffers panel.
    FindInAll,
    /// Command palette.
    Palette,
    /// Quick-open buffer switcher.
    QuickOpen,
    /// Goto-line dialog.
    GotoLine,
    /// Goto-heading picker.
    GotoHeading,
    /// §E3 font-picker palette mode.
    FontPicker,
    /// §E4 theme-picker palette mode.
    ThemePicker,
    /// §H6 Ctrl+Tab positional tab switcher.
    TabSwitcher,
    /// §H5 slash-command palette (insertion-only safelist).
    SlashPalette,
    /// Phase F3 hex-input picker (for `markdown.color_selection`).
    HexPicker,
    /// δ.4 previous-buffer browser palette mode.
    PreviousBufferBrowser,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_default() {
        let o = Overlays::idle();
        assert!(!o.is_active());
        assert_eq!(o.kind(), OverlayKind::None);
    }

    /// Phase F3 — opening the hex picker via the dedicated entry point
    /// flips the overlay state, seeds the prefill, and exposes the
    /// commit-ready predicate so the host can gate Enter.
    #[test]
    fn open_hex_picker_seeds_prefill_and_reports_commit_ready() {
        let mut o = Overlays::idle();
        o.open_hex_picker(Some("f06"));
        assert_eq!(o.kind(), OverlayKind::HexPicker);
        let hp = o.hex_picker().expect("hex picker is active");
        assert_eq!(hp.digits(), "f06");
        assert!(hp.can_commit());
    }

    /// Phase F3 — opening with a blank prefill leaves the picker
    /// open but not yet committable.
    #[test]
    fn open_hex_picker_blank_does_not_commit_yet() {
        let mut o = Overlays::idle();
        o.open_hex_picker(None);
        let hp = o.hex_picker().unwrap();
        assert_eq!(hp.digits(), "");
        assert!(!hp.can_commit());
    }

    /// Phase F3 — Esc-style dismiss returns the state to Idle so the
    /// next keystroke routes to the editor again.
    #[test]
    fn dismiss_hex_picker_returns_idle() {
        let mut o = Overlays::idle();
        o.open_hex_picker(Some("f06"));
        o.dismiss();
        assert!(!o.is_active());
        assert!(o.hex_picker().is_none());
    }

    #[test]
    fn open_then_dismiss_returns_to_idle() {
        let mut o = Overlays::idle();
        o.open(OverlayKind::Palette);
        assert_eq!(o.kind(), OverlayKind::Palette);
        assert!(o.is_active());
        o.dismiss();
        assert!(!o.is_active());
    }

    #[test]
    fn opening_a_new_kind_replaces_state() {
        let mut o = Overlays::idle();
        o.open(OverlayKind::Find);
        o.open(OverlayKind::QuickOpen);
        assert_eq!(o.kind(), OverlayKind::QuickOpen);
        assert!(o.find_bar().is_none());
    }

    #[test]
    fn open_slash_palette_installs_palette_mode_instance() {
        let mut o = Overlays::idle();
        // Generic `open()` is a no-op for the variant (data-dependent).
        o.open(OverlayKind::SlashPalette);
        assert_eq!(o.kind(), OverlayKind::None);
        // Dedicated constructor installs the variant.
        o.open_slash_palette(
            vec![crate::slash_palette::SlashPaletteEntry {
                command: "markdown.insert_toc".into(),
                label: "Insert TOC".into(),
                description: None,
                keybinding: None,
                applicable: true,
            }],
            3,
            crate::slash_palette::SlashTrigger::TypedSlash,
        );
        assert_eq!(o.kind(), OverlayKind::SlashPalette);
        assert_eq!(o.slash_palette().unwrap().anchor_line, 3);
        o.dismiss();
        assert_eq!(o.kind(), OverlayKind::None);
        assert!(o.slash_palette().is_none());
    }

    #[test]
    fn open_tab_switcher_installs_palette_mode_instance() {
        // §H6 — the generic `open(TabSwitcher)` path is a no-op
        // because the typed state needs the positional snapshot.
        // Verify the dedicated constructor installs the variant.
        let mut o = Overlays::idle();
        o.open(OverlayKind::TabSwitcher);
        assert_eq!(o.kind(), OverlayKind::None);
        // The real path takes rows + original active.
        let a = crate::pane_tree::TabId::fresh();
        let b = crate::pane_tree::TabId::fresh();
        let rows = vec![
            crate::tab_switcher::TabSwitcherRow {
                tab_id: a,
                buffer_id: continuity_buffer::BufferId::new(),
                title: "a".into(),
                subtitle: String::new(),
                dirty: false,
            },
            crate::tab_switcher::TabSwitcherRow {
                tab_id: b,
                buffer_id: continuity_buffer::BufferId::new(),
                title: "b".into(),
                subtitle: String::new(),
                dirty: false,
            },
        ];
        o.open_tab_switcher(rows, a, 1);
        assert_eq!(o.kind(), OverlayKind::TabSwitcher);
        assert_eq!(o.tab_switcher().unwrap().selected, 1);
        // Dismiss returns to idle.
        o.dismiss();
        assert_eq!(o.kind(), OverlayKind::None);
        assert!(o.tab_switcher().is_none());
    }
}
