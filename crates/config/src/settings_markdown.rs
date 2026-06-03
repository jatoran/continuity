//! `[markdown]` section of `settings.toml`, extracted from
//! [`crate::settings`] so that file stays under the 600-line cap.
//!
//! The typed path accessor [`MarkdownConfig::resolve_images_dir`] lives
//! in [`crate::markdown_paths`]; the typed-enum accessors
//! ([`crate::Settings::reveal_mode`] / [`crate::Settings::markdown_dialect`])
//! stay on `Settings` in [`crate::settings`].

use serde::Deserialize;

/// `[markdown]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MarkdownConfig {
    /// `"block" | "line"`.
    pub reveal_mode: String,
    /// Per-level heading scale multipliers.
    pub heading_scale: Vec<f32>,
    /// Phase F5: paint inline preview thumbnails for `![](url)`
    /// images. Default on per spec-delta §L#3 — image-paste is a
    /// primary path.
    pub inline_images: bool,
    /// Phase F5: shared image-store directory. Pasted / dropped
    /// images are hash-deduped into this directory; the reference in
    /// the buffer becomes `images/<hash>.<ext>` resolved against it.
    /// `%ENV%` expansion is the consumer's responsibility (matches
    /// `BackupConfig::location`).
    pub images_dir: String,
    /// Phase F7 — markdown dialect. `"gfm"` (default) enables GFM
    /// features (tables, task lists, strikethrough, autolinks) plus
    /// continuity extensions (inline color, inline table formulas).
    /// `"commonmark"` is reserved for a future strict opt-in; the
    /// renderer treats both identically until that follow-up lands.
    pub dialect: String,
    /// Render `*foo*` / `_foo_` emphasis as italic. When `false`
    /// (the default) the markup renders as literal raw text — the
    /// `*` markers stay visible and unstyled. Gates only the
    /// display-map projection + render paint, never decoration
    /// production.
    pub render_italic: bool,
    /// Render `**foo**` / `__foo__` strong as bold. Default `true`.
    /// When `false` the `**` markers stay visible and unstyled.
    pub render_bold: bool,
    /// Render `==text==` highlight backgrounds. Default `true`. When
    /// `false` the `==` markers stay visible and the highlight fill is
    /// skipped. Independent of `{#hex:}` inline color, which keeps
    /// working regardless.
    pub render_highlight: bool,
    /// Render setext heading underlines (`===` / `---` under a text
    /// line) as scaled headings. Default `true`. When `false` the
    /// heading text renders unscaled and the underline row stays
    /// literal. Distinct from `render_divider`, which governs only
    /// thematic-break rules.
    pub render_setext_heading: bool,
    /// Render `---` / `***` / `___` thematic-break divider rules.
    /// Default `true`. When `false` the literal characters stay
    /// visible and the horizontal-rule line is not painted. Does not
    /// affect setext heading underlines (see `render_setext_heading`).
    pub render_divider: bool,
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            reveal_mode: "block".into(),
            heading_scale: vec![2.0, 1.6, 1.35, 1.2, 1.1, 1.05],
            inline_images: true,
            images_dir: "%APPDATA%\\continuity\\images".into(),
            dialect: "gfm".into(),
            render_italic: false,
            render_bold: true,
            render_highlight: true,
            render_setext_heading: true,
            render_divider: true,
        }
    }
}
