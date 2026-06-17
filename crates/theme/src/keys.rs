//! The flat list of dot-keys every theme must declare. Loaders run
//! [`Theme::validate_required`](crate::Theme::validate_required) at parse
//! time so consumers can call typed accessors without `Option` plumbing.
//!
//! Keep this list in sync with spec §11. Any new key added here must be
//! present in every bundled theme (`crates/theme/assets/*.toml`) or those
//! themes will fail their parse-time validation.

/// All required theme keys in spec §11 order.
pub(crate) const REQUIRED_KEYS: &[&str] = &[
    // window
    "window.background",
    "window.foreground",
    // panel / tab strip / status bar chrome
    "panel.background",
    "panel.foreground",
    "panel.active_tab.background",
    "panel.active_tab.foreground",
    "panel.inactive_tab.background",
    "panel.inactive_tab.foreground",
    // pane chrome
    "pane.border",
    "pane.border_active",
    // editor surface
    "editor.background",
    "editor.foreground",
    "editor.cursor.primary",
    "editor.cursor.secondary",
    "editor.selection",
    "editor.selection_inactive",
    "editor.line_highlight",
    // Distinct fill for the caret's own line, separate from
    // `editor.line_highlight` (which now drives only the mouse-hover
    // band). Bundled themes set this slightly brighter / more saturated
    // than the hover band so the caret line reads as "where I am" while
    // the hover band reads as "where the pointer is."
    "editor.caret_line_highlight",
    "editor.line_number",
    "editor.line_number_active",
    "editor.indent_guide",
    "editor.indent_guide_active",
    "editor.search_match",
    "editor.search_match_active",
    "editor.find_bar.background",
    // Phase G4 search-active minimap strip (right edge of pane).
    "editor.search_minimap.background",
    "editor.search_minimap.match",
    "editor.search_minimap.match_active",
    // Scaled-text minimap (right edge of pane) — VS Code/Sublime-style
    // overview that mirrors buffer content at ~12× horizontal compression
    // so the reader can navigate by shape. Toggled by `[ui].show_minimap`.
    "editor.minimap.background",
    "editor.minimap.foreground",
    "editor.minimap.viewport_indicator",
    // P0.8.3 transient "building view" overlay drawn while paint waits
    // on a slow projection-worker build. Background is translucent so
    // the stale body reads through; foreground tints the label text.
    "editor.loading_overlay.background",
    "editor.loading_overlay.foreground",
    "editor.loading_overlay.border",
    // Phase B6 caret-jump acknowledgement glow (RGBA, low alpha).
    "editor.caret_jump_glow",
    // α.1 edit-action echo tint (RGBA, low alpha) — paste / duplicate /
    // move-line / undo-target / smart-expand pulses.
    "editor.edit_pulse",
    // Phase B8 rainbow bracket-pair palette (6 levels, cycled by depth).
    "editor.pair_rainbow.0",
    "editor.pair_rainbow.1",
    "editor.pair_rainbow.2",
    "editor.pair_rainbow.3",
    "editor.pair_rainbow.4",
    "editor.pair_rainbow.5",
    // Phase B17 soft-wrap continuation glyph color.
    "editor.soft_wrap_indicator",
    // Phase F1 sticky heading breadcrumb (top-of-pane heading chain).
    "editor.breadcrumb.foreground",
    "editor.breadcrumb.separator",
    "editor.breadcrumb.active",
    // Phase F2 outline sidebar (right-docked heading tree).
    "editor.outline.background",
    "editor.outline.foreground",
    "editor.outline.foreground_active",
    "editor.outline.separator",
    // Phase F3 inline highlight markup (`==text==`).
    "editor.inline_highlight.foreground",
    "editor.inline_highlight.background",
    // Phase H1 granular focus-mode dim alpha (single scalar `#aa` color
    // — only the alpha channel is consumed; RGB ignored).
    "editor.focus_dim_alpha",
    // Phase H2 distraction-free mode dimmed foreground for non-current
    // paragraphs.
    "editor.foreground_dim",
    // markdown headings 1..6
    "markdown.heading.1",
    "markdown.heading.2",
    "markdown.heading.3",
    "markdown.heading.4",
    "markdown.heading.5",
    "markdown.heading.6",
    // markdown inline emphasis
    "markdown.bold",
    "markdown.italic",
    "markdown.strikethrough",
    // markdown code
    "markdown.code.foreground",
    "markdown.code.background",
    "markdown.code_block.background",
    "markdown.code_block.border",
    // markdown blockquote
    "markdown.blockquote.foreground",
    "markdown.blockquote.bar",
    // markdown links / images
    "markdown.link",
    "markdown.footnote",
    "markdown.url",
    "markdown.image_alt",
    // markdown list / checkbox / hr / table
    "markdown.list_marker",
    "markdown.checkbox.checked",
    "markdown.checkbox.unchecked",
    "markdown.hr",
    "markdown.table.border",
    "markdown.table.header_bg",
    "markdown.table.alignment_bg",
    "markdown.table.active_cell_outline",
    // Phase F4 inline-formula swap-in (computed-value text + error sentinel).
    "markdown.formula.value",
    "markdown.formula.error",
    // status bar
    "status.background",
    "status.foreground",
    "status.error",
    "status.warn",
    "status.info",
    // overlay (palette / quick-open / banners)
    "overlay.background",
    "overlay.shadow",
    // command palette
    "palette.background",
    "palette.match_highlight",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn required_keys_are_unique() {
        let set: HashSet<&&str> = REQUIRED_KEYS.iter().collect();
        assert_eq!(set.len(), REQUIRED_KEYS.len());
    }

    #[test]
    fn required_keys_have_expected_count() {
        // Sanity: roughly the spec §11 count. If you add a key, update this.
        assert!(REQUIRED_KEYS.len() >= 50);
        assert!(REQUIRED_KEYS.len() < 100);
    }

    #[test]
    fn focus_mode_keys_registered() {
        assert!(REQUIRED_KEYS.contains(&"editor.focus_dim_alpha"));
        assert!(REQUIRED_KEYS.contains(&"editor.foreground_dim"));
    }
}
