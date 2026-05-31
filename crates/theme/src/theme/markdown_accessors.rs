//! [`Theme`] typed accessors for the `markdown.*` key namespace.
//! Hoisted out of the parent `theme.rs` to keep that file under the
//! 600-line cap; the surface and semantics are unchanged.

use crate::theme::Theme;
use crate::Color;

impl Theme {
    /// `markdown.heading.{level}` for `level` in `1..=6`. Out-of-range
    /// values clamp to the nearest valid level.
    #[must_use]
    pub fn markdown_heading(&self, level: u8) -> Color {
        let key = match level.clamp(1, 6) {
            1 => "markdown.heading.1",
            2 => "markdown.heading.2",
            3 => "markdown.heading.3",
            4 => "markdown.heading.4",
            5 => "markdown.heading.5",
            _ => "markdown.heading.6",
        };
        self.required(key)
    }
    /// `markdown.bold`.
    #[must_use]
    pub fn markdown_bold(&self) -> Color {
        self.required("markdown.bold")
    }
    /// `markdown.italic`.
    #[must_use]
    pub fn markdown_italic(&self) -> Color {
        self.required("markdown.italic")
    }
    /// `markdown.strikethrough`.
    #[must_use]
    pub fn markdown_strikethrough(&self) -> Color {
        self.required("markdown.strikethrough")
    }
    /// `markdown.code.foreground`.
    #[must_use]
    pub fn markdown_code_foreground(&self) -> Color {
        self.required("markdown.code.foreground")
    }
    /// `markdown.code.background`.
    #[must_use]
    pub fn markdown_code_background(&self) -> Color {
        self.required("markdown.code.background")
    }
    /// `markdown.code_block.background`.
    #[must_use]
    pub fn markdown_code_block_background(&self) -> Color {
        self.required("markdown.code_block.background")
    }
    /// `markdown.code_block.border`.
    #[must_use]
    pub fn markdown_code_block_border(&self) -> Color {
        self.required("markdown.code_block.border")
    }
    /// `markdown.blockquote.foreground`.
    #[must_use]
    pub fn markdown_blockquote_foreground(&self) -> Color {
        self.required("markdown.blockquote.foreground")
    }
    /// `markdown.blockquote.bar`.
    #[must_use]
    pub fn markdown_blockquote_bar(&self) -> Color {
        self.required("markdown.blockquote.bar")
    }
    /// `markdown.link`.
    #[must_use]
    pub fn markdown_link(&self) -> Color {
        self.required("markdown.link")
    }
    /// `markdown.footnote`.
    #[must_use]
    pub fn markdown_footnote(&self) -> Color {
        self.required("markdown.footnote")
    }
    /// `markdown.url`.
    #[must_use]
    pub fn markdown_url(&self) -> Color {
        self.required("markdown.url")
    }
    /// `markdown.image_alt`.
    #[must_use]
    pub fn markdown_image_alt(&self) -> Color {
        self.required("markdown.image_alt")
    }
    /// `markdown.list_marker`.
    #[must_use]
    pub fn markdown_list_marker(&self) -> Color {
        self.required("markdown.list_marker")
    }
    /// `markdown.checkbox.checked`.
    #[must_use]
    pub fn markdown_checkbox_checked(&self) -> Color {
        self.required("markdown.checkbox.checked")
    }
    /// `markdown.checkbox.unchecked`.
    #[must_use]
    pub fn markdown_checkbox_unchecked(&self) -> Color {
        self.required("markdown.checkbox.unchecked")
    }
    /// `markdown.hr`.
    #[must_use]
    pub fn markdown_hr(&self) -> Color {
        self.required("markdown.hr")
    }
    /// `markdown.table.border`.
    #[must_use]
    pub fn markdown_table_border(&self) -> Color {
        self.required("markdown.table.border")
    }

    /// `markdown.table.header_bg` — subtle fill behind a pipe-table
    /// header row when the visual-table renderer is drawing the block.
    #[must_use]
    pub fn markdown_table_header_bg(&self) -> Color {
        self.required("markdown.table.header_bg")
    }

    /// `markdown.table.alignment_bg` — fill behind the pipe-table
    /// alignment-row slot (`|---|---|`). Distinct from `header_bg` so
    /// the divider strip reads as its own band between the header and
    /// the body.
    #[must_use]
    pub fn markdown_table_alignment_bg(&self) -> Color {
        self.required("markdown.table.alignment_bg")
    }

    /// `markdown.table.active_cell_outline` — stroke color for the
    /// active (caret-containing / selected) pipe-table cell outline and
    /// its translucent fill. Themed independently of the editor caret.
    #[must_use]
    pub fn markdown_table_active_cell_outline(&self) -> Color {
        self.required("markdown.table.active_cell_outline")
    }

    /// `markdown.formula.value` — Phase F4 swap-in foreground for the
    /// computed value of a table-cell formula.
    #[must_use]
    pub fn markdown_formula_value(&self) -> Color {
        self.required("markdown.formula.value")
    }

    /// `markdown.formula.error` — Phase F4 foreground for `#DIV/0!` /
    /// `#ERR` sentinels.
    #[must_use]
    pub fn markdown_formula_error(&self) -> Color {
        self.required("markdown.formula.error")
    }
}
