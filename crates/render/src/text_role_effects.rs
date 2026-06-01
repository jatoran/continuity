//! DirectWrite foreground drawing effects for styled text roles.

use continuity_decorate::HighlightKind;
use continuity_display_map::{SpanRole, SpanStyle};
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{ID2D1RenderTarget, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{IDWriteTextLayout, DWRITE_TEXT_RANGE};

use crate::params::colors::MarkdownColors;
use crate::params::Rgba;
use crate::Error;

/// Owned brush set for styled text roles.
pub(crate) struct TextRoleBrushSet {
    code: ID2D1SolidColorBrush,
    footnote: ID2D1SolidColorBrush,
    link: ID2D1SolidColorBrush,
    syntax_keyword: ID2D1SolidColorBrush,
    syntax_type: ID2D1SolidColorBrush,
    syntax_string: ID2D1SolidColorBrush,
    syntax_number: ID2D1SolidColorBrush,
    syntax_comment: ID2D1SolidColorBrush,
    syntax_function: ID2D1SolidColorBrush,
    syntax_punctuation: ID2D1SolidColorBrush,
}

/// Borrowed brush view passed into the per-line painters.
#[derive(Clone, Copy)]
pub(crate) struct TextRoleBrushes<'a> {
    /// Inline/fenced code foreground.
    pub(crate) code: &'a ID2D1SolidColorBrush,
    /// Footnote foreground.
    pub(crate) footnote: &'a ID2D1SolidColorBrush,
    /// Markdown link foreground (`markdown.link`).
    pub(crate) link: &'a ID2D1SolidColorBrush,
    /// Syntax keyword foreground.
    pub(crate) syntax_keyword: &'a ID2D1SolidColorBrush,
    /// Syntax type/key foreground.
    pub(crate) syntax_type: &'a ID2D1SolidColorBrush,
    /// Syntax string foreground.
    pub(crate) syntax_string: &'a ID2D1SolidColorBrush,
    /// Syntax number foreground.
    pub(crate) syntax_number: &'a ID2D1SolidColorBrush,
    /// Syntax comment foreground.
    pub(crate) syntax_comment: &'a ID2D1SolidColorBrush,
    /// Syntax function foreground.
    pub(crate) syntax_function: &'a ID2D1SolidColorBrush,
    /// Syntax punctuation foreground.
    pub(crate) syntax_punctuation: &'a ID2D1SolidColorBrush,
}

impl TextRoleBrushSet {
    /// Create the owned brush set for one frame's active theme.
    pub(crate) fn new(
        render_target: &ID2D1RenderTarget,
        markdown: &MarkdownColors,
        editor_fg: Rgba,
    ) -> Result<Self, Error> {
        let mkb = |rgba: Rgba| -> Result<ID2D1SolidColorBrush, Error> {
            Ok(unsafe { render_target.CreateSolidColorBrush(&D2D1_COLOR_F::from(rgba), None)? })
        };
        Ok(Self {
            code: mkb(fallback(markdown.code_fg, editor_fg))?,
            footnote: mkb(fallback(markdown.footnote, editor_fg))?,
            link: mkb(fallback(markdown.link, editor_fg))?,
            syntax_keyword: mkb(syntax_color(markdown, editor_fg, HighlightKind::Keyword))?,
            syntax_type: mkb(syntax_color(markdown, editor_fg, HighlightKind::Type))?,
            syntax_string: mkb(syntax_color(markdown, editor_fg, HighlightKind::String))?,
            syntax_number: mkb(syntax_color(markdown, editor_fg, HighlightKind::Number))?,
            syntax_comment: mkb(syntax_color(markdown, editor_fg, HighlightKind::Comment))?,
            syntax_function: mkb(syntax_color(markdown, editor_fg, HighlightKind::Function))?,
            syntax_punctuation: mkb(syntax_color(
                markdown,
                editor_fg,
                HighlightKind::Punctuation,
            ))?,
        })
    }

    /// Borrow the owned brushes for per-line painter structs.
    pub(crate) fn refs(&self) -> TextRoleBrushes<'_> {
        TextRoleBrushes {
            code: &self.code,
            footnote: &self.footnote,
            link: &self.link,
            syntax_keyword: &self.syntax_keyword,
            syntax_type: &self.syntax_type,
            syntax_string: &self.syntax_string,
            syntax_number: &self.syntax_number,
            syntax_comment: &self.syntax_comment,
            syntax_function: &self.syntax_function,
            syntax_punctuation: &self.syntax_punctuation,
        }
    }
}

/// Apply foreground drawing effects to role-tagged text-layout runs.
pub(crate) fn apply_role_drawing_effects(
    layout: &IDWriteTextLayout,
    runs: impl Iterator<
        Item = (
            std::ops::Range<continuity_display_map::DisplayUtf16>,
            SpanStyle,
        ),
    >,
    brushes: &TextRoleBrushes<'_>,
) {
    for (range, style) in runs {
        let Some(brush) = role_brush(style.role, brushes) else {
            continue;
        };
        let Ok(effect) = brush.cast::<windows::core::IUnknown>() else {
            continue;
        };
        let start = range.start.raw();
        let length = range.end.raw().saturating_sub(start);
        if length == 0 {
            continue;
        }
        let dwrite_range = DWRITE_TEXT_RANGE {
            startPosition: start,
            length,
        };
        unsafe {
            let _ = layout.SetDrawingEffect(&effect, dwrite_range);
        }
    }
}

fn fallback(color: Rgba, fallback: Rgba) -> Rgba {
    if color.a > 0.0 {
        color
    } else {
        fallback
    }
}

fn syntax_color(markdown: &MarkdownColors, editor_fg: Rgba, kind: HighlightKind) -> Rgba {
    let picked = match kind {
        HighlightKind::Keyword => markdown.link,
        HighlightKind::Type => markdown.heading[2],
        HighlightKind::String => markdown.code_fg,
        HighlightKind::Number => markdown.formula_value,
        HighlightKind::Comment => markdown.blockquote_fg,
        HighlightKind::Function => markdown.heading[3],
        HighlightKind::Punctuation => markdown.url,
    };
    fallback(picked, editor_fg)
}

fn role_brush<'brush>(
    role: SpanRole,
    brushes: &TextRoleBrushes<'brush>,
) -> Option<&'brush ID2D1SolidColorBrush> {
    match role {
        SpanRole::Code => Some(brushes.code),
        SpanRole::Footnote => Some(brushes.footnote),
        SpanRole::Link => Some(brushes.link),
        SpanRole::Syntax(HighlightKind::Keyword) => Some(brushes.syntax_keyword),
        SpanRole::Syntax(HighlightKind::Type) => Some(brushes.syntax_type),
        SpanRole::Syntax(HighlightKind::String) => Some(brushes.syntax_string),
        SpanRole::Syntax(HighlightKind::Number) => Some(brushes.syntax_number),
        SpanRole::Syntax(HighlightKind::Comment) => Some(brushes.syntax_comment),
        SpanRole::Syntax(HighlightKind::Function) => Some(brushes.syntax_function),
        SpanRole::Syntax(HighlightKind::Punctuation) => Some(brushes.syntax_punctuation),
        _ => None,
    }
}
