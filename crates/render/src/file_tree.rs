//! Pure draw payload for the left file-tree pane.

use crate::params::Rgba;

/// Default left file-tree pane width.
pub const FILE_TREE_DEFAULT_WIDTH_DIP: f32 = 280.0;
/// Header height for the opened folder label.
pub const FILE_TREE_HEADER_HEIGHT_DIP: f32 = 30.0;
/// Row height for one file-tree entry.
pub const FILE_TREE_ROW_HEIGHT_DIP: f32 = 22.0;

/// File-tree paint payload.
#[derive(Clone, Debug)]
pub struct FileTreeDraw {
    /// Pane rect `(x, y, w, h)` in client DIPs.
    pub rect: (f32, f32, f32, f32),
    /// Header title, normally the opened root directory name.
    pub title: String,
    /// Visible rows only, already clipped by the UI to viewport + overscan.
    pub rows: Vec<FileTreeRowDraw>,
    /// Theme-derived colors.
    pub colors: FileTreeColors,
    /// Absolute index of `rows[0]` in the full visible tree.
    pub first_row_index: u32,
    /// Row height in DIPs.
    pub row_height_dip: f32,
    /// Header height in DIPs.
    pub header_height_dip: f32,
    /// Current vertical scroll offset.
    pub scroll_offset_dip: f32,
    /// Full visible-tree content height.
    pub content_height_dip: f32,
    /// In-flight entry move feedback, if the pointer crossed the drag threshold.
    pub drag: Option<FileTreeDragDraw>,
}

/// Visual feedback for a contained file-tree move.
#[derive(Clone, Debug)]
pub struct FileTreeDragDraw {
    /// Source entry label painted beside the pointer.
    pub label: String,
    /// Current pointer position in client DIPs.
    pub cursor: (f32, f32),
    /// Top of the hovered destination row, when the pointer is over a row.
    pub drop_target_top_dip: Option<f32>,
}

/// One visible file-tree row.
#[derive(Clone, Debug)]
pub struct FileTreeRowDraw {
    /// Display label.
    pub label: String,
    /// Nesting depth from the opened root.
    pub depth: u16,
    /// Entry kind.
    pub kind: FileTreeEntryKind,
    /// Expanded state for directories.
    pub expanded: bool,
    /// Selected row flag.
    pub selected: bool,
    /// Loading flag for directories waiting on worker listing.
    pub loading: bool,
    /// Optional vault-config label color.
    pub color_override: Option<Rgba>,
    /// Inline filename editor replacing the normal label for this row.
    pub inline_edit: Option<FileTreeInlineEditDraw>,
}

/// Single-line inline editor state for one file-tree row.
#[derive(Clone, Debug)]
pub struct FileTreeInlineEditDraw {
    /// Actual file or folder name, including its extension.
    pub text: String,
    /// UTF-8 byte offset of the caret.
    pub caret_byte: usize,
    /// Selected UTF-8 byte range, when non-empty.
    pub selection_range: Option<(usize, usize)>,
}

/// File-tree row kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileTreeEntryKind {
    /// Directory row.
    Directory,
    /// File row.
    File,
    /// Informational row.
    Notice,
}

/// File-tree colors.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileTreeColors {
    /// Background fill.
    pub bg: Rgba,
    /// Normal foreground.
    pub fg: Rgba,
    /// Muted foreground.
    pub muted: Rgba,
    /// Directory foreground.
    pub folder_fg: Rgba,
    /// Selected row background.
    pub selected_bg: Rgba,
    /// Separator rule.
    pub separator: Rgba,
}
