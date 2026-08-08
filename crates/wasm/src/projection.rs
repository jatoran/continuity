//! Synchronous Markdown decoration and display-projection reporting.

use continuity_buffer::RopeSnapshot;
use continuity_decorate::{BlockKind, Decorations, InlineKind, MarkerKind};
use continuity_display_map::wrap::FixedCharWidth;
use continuity_display_map::{
    DisplayByte, DisplayMapBuilder, DisplaySegment, SourceByte, WrapConfig,
};
use serde::Serialize;

/// One decoration span fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanReport {
    /// Portable kind name.
    pub kind: &'static str,
    /// Inclusive source byte.
    pub start_byte: usize,
    /// Exclusive source byte.
    pub end_byte: usize,
}

/// One projected display segment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentReport {
    /// `visible`, `hidden`, or `replace`.
    pub kind: &'static str,
    /// Inclusive source byte.
    pub start_byte: u32,
    /// Exclusive source byte.
    pub end_byte: u32,
    /// Replacement text; empty for visible and hidden segments.
    pub replacement: String,
}

/// One fully realized display line and its bidirectional byte mappings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayLineReport {
    /// Projected line text.
    pub text: String,
    /// Display-byte end of the exact prefix used as the hanging indent.
    pub wrap_indent_byte_end: usize,
    /// Source-to-display byte pairs.
    pub source_to_display: Vec<[u32; 2]>,
    /// Display-to-source byte pairs.
    pub display_to_source: Vec<[u32; 2]>,
    /// Ordered projected segments.
    pub segments: Vec<SegmentReport>,
}

/// Portable decoration and display-map parity result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionReport {
    /// Block-level decoration spans.
    pub blocks: Vec<SpanReport>,
    /// Inline decoration spans.
    pub inlines: Vec<SpanReport>,
    /// Fully realized display lines.
    pub lines: Vec<DisplayLineReport>,
}

/// Viewport-scoped presentation update for an existing browser projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationRangeReport {
    /// First source line represented by `lines`.
    pub start_line: u32,
    /// Exclusive source line represented by `lines`.
    pub end_line: u32,
    /// Total source-line count after the edit.
    pub line_count: u32,
    /// Absolute UTF-8 byte start for each represented source line.
    pub line_starts: Vec<usize>,
    /// Block spans intersecting the represented range.
    pub blocks: Vec<SpanReport>,
    /// Inline spans intersecting the represented range.
    pub inlines: Vec<SpanReport>,
    /// Projected lines in source-line order.
    pub lines: Vec<DisplayLineReport>,
}

/// Compute synchronous Markdown decoration and a no-wrap display map.
pub fn compute_projection_report(snapshot: &RopeSnapshot) -> Result<ProjectionReport, String> {
    compute_report(snapshot, true)
}

/// Compute the compact projection consumed by browser presentation code.
pub fn compute_presentation_report(snapshot: &RopeSnapshot) -> Result<ProjectionReport, String> {
    compute_report(snapshot, false)
}

fn compute_report(
    snapshot: &RopeSnapshot,
    should_include_mappings: bool,
) -> Result<ProjectionReport, String> {
    let source = snapshot.rope().to_string();
    let decorations = Decorations::compute(&source, snapshot.revision().get())
        .ok_or_else(|| "Markdown parser did not return a tree".to_string())?;
    compute_report_with_decorations(snapshot, &decorations, should_include_mappings)
}

pub(crate) fn compute_report_with_decorations(
    snapshot: &RopeSnapshot,
    decorations: &Decorations,
    should_include_mappings: bool,
) -> Result<ProjectionReport, String> {
    let mut measure = FixedCharWidth::default();
    let map = DisplayMapBuilder::new(snapshot, decorations, &[], &[], WrapConfig::NONE)
        .build(&mut measure)
        .map_err(|error| error.to_string())?;
    let blocks = decorations
        .blocks
        .iter()
        .map(|span| SpanReport {
            kind: block_kind_name(span.kind),
            start_byte: span.start_byte,
            end_byte: span.end_byte,
        })
        .collect();
    let inlines = decorations
        .inlines
        .iter()
        .map(|span| SpanReport {
            kind: inline_kind_name(&span.kind),
            start_byte: span.range.start,
            end_byte: span.range.end,
        })
        .collect();
    let lines = map
        .display_lines()
        .map(|line| display_line_report(line, should_include_mappings))
        .collect();
    Ok(ProjectionReport {
        blocks,
        inlines,
        lines,
    })
}

pub(crate) fn compute_range_report_with_decorations(
    snapshot: &RopeSnapshot,
    decorations: &Decorations,
    requested_start: u32,
    requested_end: u32,
) -> Result<PresentationRangeReport, String> {
    let rope = snapshot.rope();
    let line_count = rope.len_lines() as u32;
    let start_line = requested_start.min(line_count);
    let end_line = requested_end.min(line_count).max(start_line);
    let start_byte = rope.line_to_byte(start_line as usize);
    let end_byte = if end_line < line_count {
        rope.line_to_byte(end_line as usize)
    } else {
        rope.len_bytes()
    };
    let mut measure = FixedCharWidth::default();
    let lines = DisplayMapBuilder::new(snapshot, decorations, &[], &[], WrapConfig::NONE)
        .build_source_lines(start_line..end_line, &mut measure)
        .map_err(|error| error.to_string())?
        .iter()
        .map(|line| display_line_report(line, false))
        .collect();
    let line_starts = (start_line..end_line)
        .map(|line| rope.line_to_byte(line as usize))
        .collect();
    let blocks = decorations
        .blocks
        .iter()
        .filter(|span| span.end_byte > start_byte && span.start_byte < end_byte)
        .map(|span| SpanReport {
            kind: block_kind_name(span.kind),
            start_byte: span.start_byte,
            end_byte: span.end_byte,
        })
        .collect();
    let inlines = decorations
        .inlines
        .iter()
        .filter(|span| span.range.end > start_byte && span.range.start < end_byte)
        .map(|span| SpanReport {
            kind: inline_kind_name(&span.kind),
            start_byte: span.range.start,
            end_byte: span.range.end,
        })
        .collect();
    Ok(PresentationRangeReport {
        start_line,
        end_line,
        line_count,
        line_starts,
        blocks,
        inlines,
        lines,
    })
}

fn display_line_report(
    line: &continuity_display_map::DisplayLineSpec,
    should_include_mappings: bool,
) -> DisplayLineReport {
    let source_to_display = if should_include_mappings {
        (line.source_byte_start.raw()..=line.source_byte_end.raw())
            .filter_map(|source| {
                line.source_to_display(SourceByte(source))
                    .map(|display| [source, display.raw()])
            })
            .collect()
    } else {
        Vec::new()
    };
    let display_to_source = if should_include_mappings {
        (0..=line.display_text().len() as u32)
            .filter_map(|display| {
                line.display_to_source(DisplayByte(display))
                    .map(|source| [display, source.raw()])
            })
            .collect()
    } else {
        Vec::new()
    };
    DisplayLineReport {
        text: line.display_text().to_string(),
        wrap_indent_byte_end: continuity_display_map::wrap::hanging_indent_display_byte_end(
            line.display_text(),
        ),
        source_to_display,
        display_to_source,
        segments: line.segments.iter().map(segment_report).collect(),
    }
}

fn segment_report(segment: &DisplaySegment) -> SegmentReport {
    let source = segment.source_range();
    let (kind, replacement) = match segment {
        DisplaySegment::Visible { .. } => ("visible", String::new()),
        DisplaySegment::Hidden { .. } => ("hidden", String::new()),
        DisplaySegment::Replace { display, .. } => ("replace", display.to_string()),
    };
    SegmentReport {
        kind,
        start_byte: source.start.raw(),
        end_byte: source.end.raw(),
        replacement,
    }
}

fn block_kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Heading { level: 1 } => "heading:1",
        BlockKind::Heading { level: 2 } => "heading:2",
        BlockKind::Heading { level: 3 } => "heading:3",
        BlockKind::Heading { level: 4 } => "heading:4",
        BlockKind::Heading { level: 5 } => "heading:5",
        BlockKind::Heading { level: 6 } => "heading:6",
        BlockKind::Heading { .. } => "heading:6",
        BlockKind::SetextHeading { level: 1 } => "heading:1",
        BlockKind::SetextHeading { .. } => "heading:2",
        BlockKind::Paragraph => "paragraph",
        BlockKind::FencedCodeBlock => "fencedCodeBlock",
        BlockKind::IndentedCodeBlock => "indentedCodeBlock",
        BlockKind::List => "list",
        BlockKind::ListItem => "listItem",
        BlockKind::BlockQuote => "blockQuote",
        BlockKind::HorizontalRule => "horizontalRule",
        BlockKind::PipeTable => "pipeTable",
        BlockKind::HtmlBlock => "htmlBlock",
        BlockKind::Other(_) => "other",
    }
}

fn inline_kind_name(kind: &InlineKind) -> &'static str {
    match kind {
        InlineKind::Strong => "strong",
        InlineKind::Emphasis => "emphasis",
        InlineKind::Strikethrough => "strikethrough",
        InlineKind::Code => "code",
        InlineKind::Link { .. } => "link",
        InlineKind::FootnoteReference { .. } => "footnoteReference",
        InlineKind::FootnoteDefinition { .. } => "footnoteDefinition",
        InlineKind::ImageRef { .. } => "imageRef",
        InlineKind::Checkbox { .. } => "checkbox",
        InlineKind::Marker(MarkerKind::HeadingHash) => "marker:heading",
        InlineKind::Marker(MarkerKind::EmphasisDelim) => "marker:emphasis",
        InlineKind::Marker(_) => "marker",
    }
}
