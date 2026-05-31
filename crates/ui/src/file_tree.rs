//! UI-thread state for the left file-tree pane.
//!
//! The tree owns no disk handles and performs no filesystem walking.
//! It stores the bounded directory listings delivered by the file-I/O
//! worker and projects the currently visible rows for paint/hit-test.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use continuity_render::{
    EditorColors, FileTreeColors, FileTreeDraw, FileTreeEntryKind, FileTreeRowDraw,
    FILE_TREE_DEFAULT_WIDTH_DIP, FILE_TREE_HEADER_HEIGHT_DIP, FILE_TREE_ROW_HEIGHT_DIP,
};

use crate::{DirectoryEntry, DirectoryEntryKind};

const FILE_TREE_MAX_TOTAL_ROWS: usize = 50_000;
const FILE_TREE_PAINT_OVERSCAN_ROWS: usize = 4;

/// Maximum file size opened directly from the tree.
pub(crate) const FILE_TREE_MAX_OPEN_BYTES: u64 = 8 * 1024 * 1024;

/// Window-owned file-tree state.
#[derive(Debug, Default)]
pub(crate) struct FileTreeState {
    root: Option<PathBuf>,
    visible: bool,
    nodes: HashMap<PathBuf, FileTreeNode>,
    pending: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    scroll_offset_dip: f32,
    hit_rows: Vec<FileTreeHitRow>,
}

#[derive(Clone, Debug)]
struct FileTreeNode {
    name: String,
    kind: FileTreeNodeKind,
    relative: PathBuf,
    size_bytes: Option<u64>,
    expanded: bool,
    loaded: bool,
    truncated: bool,
    children: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileTreeNodeKind {
    Directory,
    File,
}

#[derive(Clone, Debug)]
struct VisibleRow {
    relative: PathBuf,
    label: String,
    depth: u16,
    kind: FileTreeEntryKind,
    expanded: bool,
    selected: bool,
    loading: bool,
    size_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct FileTreeHitRow {
    pub(crate) relative: PathBuf,
    pub(crate) kind: FileTreeEntryKind,
    pub(crate) size_bytes: Option<u64>,
}

impl FileTreeState {
    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn visible_width_dip(&self) -> f32 {
        if self.visible {
            FILE_TREE_DEFAULT_WIDTH_DIP
        } else {
            0.0
        }
    }

    pub(crate) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            self.hit_rows.clear();
        }
    }

    pub(crate) fn open_root(&mut self, root: PathBuf) -> PathBuf {
        self.root = Some(root.clone());
        self.visible = true;
        self.nodes.clear();
        self.pending.clear();
        self.selected = None;
        self.scroll_offset_dip = 0.0;
        self.hit_rows.clear();
        let relative = PathBuf::new();
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| root.display().to_string());
        self.nodes.insert(
            relative.clone(),
            FileTreeNode {
                name,
                kind: FileTreeNodeKind::Directory,
                relative: relative.clone(),
                size_bytes: None,
                expanded: true,
                loaded: false,
                truncated: false,
                children: Vec::new(),
            },
        );
        self.pending.insert(relative.clone());
        relative
    }

    pub(crate) fn mark_pending(&mut self, relative: PathBuf) {
        self.pending.insert(relative);
    }

    pub(crate) fn clear_pending(&mut self, relative: &Path) {
        self.pending.remove(relative);
    }

    pub(crate) fn apply_directory_list(
        &mut self,
        root: &Path,
        relative: PathBuf,
        entries: Vec<DirectoryEntry>,
        truncated: bool,
    ) -> bool {
        if self.root.as_deref() != Some(root) {
            return false;
        }
        self.pending.remove(&relative);
        let mut children = Vec::new();
        let mut child_nodes = Vec::new();
        for entry in entries {
            let kind = match entry.kind {
                DirectoryEntryKind::Directory => FileTreeNodeKind::Directory,
                DirectoryEntryKind::File => FileTreeNodeKind::File,
            };
            children.push(entry.relative.clone());
            child_nodes.push((
                entry.relative.clone(),
                FileTreeNode {
                    name: entry.name,
                    kind,
                    relative: entry.relative,
                    size_bytes: entry.size_bytes,
                    expanded: false,
                    loaded: kind == FileTreeNodeKind::File,
                    truncated: false,
                    children: Vec::new(),
                },
            ));
        }
        let Some(parent) = self.nodes.get_mut(&relative) else {
            return false;
        };
        parent.loaded = true;
        parent.truncated = truncated;
        parent.children = children;
        for (relative, node) in child_nodes {
            self.nodes.insert(relative, node);
        }
        true
    }

    pub(crate) fn toggle_directory(&mut self, relative: &Path) -> Option<PathBuf> {
        let node = self.nodes.get_mut(relative)?;
        if node.kind != FileTreeNodeKind::Directory {
            return None;
        }
        node.expanded = !node.expanded;
        if node.expanded && !node.loaded {
            let relative = node.relative.clone();
            self.pending.insert(relative.clone());
            return Some(relative);
        }
        None
    }

    pub(crate) fn select(&mut self, relative: PathBuf) {
        self.selected = Some(relative);
    }

    pub(crate) fn absolute_path(&self, relative: &Path) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join(relative))
    }

    pub(crate) fn row_at(&self, x: f32, y: f32) -> Option<FileTreeHitRow> {
        if !self.visible || !(0.0..FILE_TREE_DEFAULT_WIDTH_DIP).contains(&x) {
            return None;
        }
        if y < FILE_TREE_HEADER_HEIGHT_DIP {
            return None;
        }
        let row = ((y - FILE_TREE_HEADER_HEIGHT_DIP) / FILE_TREE_ROW_HEIGHT_DIP).floor();
        if row < 0.0 {
            return None;
        }
        self.hit_rows.get(row as usize).cloned()
    }

    pub(crate) fn scroll_by_notches(&mut self, notches: f32, viewport_height_dip: f32) -> bool {
        if !self.visible {
            return false;
        }
        let before = self.scroll_offset_dip;
        let rows = self.collect_rows();
        let content_height = rows.len() as f32 * FILE_TREE_ROW_HEIGHT_DIP;
        let viewport = (viewport_height_dip - FILE_TREE_HEADER_HEIGHT_DIP).max(1.0);
        let max_scroll = (content_height - viewport).max(0.0);
        self.scroll_offset_dip = (self.scroll_offset_dip
            - notches * 3.0 * FILE_TREE_ROW_HEIGHT_DIP)
            .clamp(0.0, max_scroll);
        (self.scroll_offset_dip - before).abs() > f32::EPSILON
    }

    pub(crate) fn build_draw(
        &mut self,
        client_height_dip: f32,
        colors: EditorColors,
    ) -> Option<FileTreeDraw> {
        if !self.visible {
            return None;
        }
        let rows = self.collect_rows();
        let viewport = (client_height_dip - FILE_TREE_HEADER_HEIGHT_DIP).max(1.0);
        let content_height = rows.len() as f32 * FILE_TREE_ROW_HEIGHT_DIP;
        let max_scroll = (content_height - viewport).max(0.0);
        self.scroll_offset_dip = self.scroll_offset_dip.clamp(0.0, max_scroll);
        let first = (self.scroll_offset_dip / FILE_TREE_ROW_HEIGHT_DIP).floor() as usize;
        let visible_count =
            (viewport / FILE_TREE_ROW_HEIGHT_DIP).ceil() as usize + FILE_TREE_PAINT_OVERSCAN_ROWS;
        self.hit_rows.clear();
        let mut draw_rows = Vec::with_capacity(visible_count.min(rows.len()));
        for row in rows.iter().skip(first).take(visible_count) {
            self.hit_rows.push(FileTreeHitRow {
                relative: row.relative.clone(),
                kind: row.kind,
                size_bytes: row.size_bytes,
            });
            draw_rows.push(FileTreeRowDraw {
                label: row.label.clone(),
                depth: row.depth,
                kind: row.kind,
                expanded: row.expanded,
                selected: row.selected,
                loading: row.loading,
            });
        }
        let title = self
            .root
            .as_ref()
            .map(|root| root.display().to_string())
            .unwrap_or_else(|| "No folder".into());
        Some(FileTreeDraw {
            rect: (0.0, 0.0, FILE_TREE_DEFAULT_WIDTH_DIP, client_height_dip),
            title,
            rows: draw_rows,
            colors: file_tree_colors(colors),
            first_row_index: first as u32,
            row_height_dip: FILE_TREE_ROW_HEIGHT_DIP,
            header_height_dip: FILE_TREE_HEADER_HEIGHT_DIP,
            scroll_offset_dip: self.scroll_offset_dip,
            content_height_dip: content_height,
        })
    }

    fn collect_rows(&self) -> Vec<VisibleRow> {
        let mut rows = Vec::new();
        let root_relative = PathBuf::new();
        let Some(root) = self.nodes.get(&root_relative) else {
            return rows;
        };
        if self.pending.contains(&root_relative) && !root.loaded {
            rows.push(notice_row("Loading folder...", 0));
            return rows;
        }
        self.collect_children(root, 0, &mut rows);
        if root.truncated && rows.len() < FILE_TREE_MAX_TOTAL_ROWS {
            rows.push(notice_row("More entries hidden by safety cap", 0));
        }
        rows
    }

    fn collect_children(&self, node: &FileTreeNode, depth: u16, rows: &mut Vec<VisibleRow>) {
        if rows.len() >= FILE_TREE_MAX_TOTAL_ROWS {
            return;
        }
        for child_relative in &node.children {
            let Some(child) = self.nodes.get(child_relative) else {
                continue;
            };
            rows.push(VisibleRow {
                relative: child.relative.clone(),
                label: child.name.clone(),
                depth,
                kind: match child.kind {
                    FileTreeNodeKind::Directory => FileTreeEntryKind::Directory,
                    FileTreeNodeKind::File => FileTreeEntryKind::File,
                },
                expanded: child.expanded,
                selected: self.selected.as_ref() == Some(&child.relative),
                loading: self.pending.contains(&child.relative),
                size_bytes: child.size_bytes,
            });
            if child.kind == FileTreeNodeKind::Directory && child.expanded {
                if self.pending.contains(&child.relative) && !child.loaded {
                    rows.push(notice_row("Loading...", depth.saturating_add(1)));
                } else {
                    self.collect_children(child, depth.saturating_add(1), rows);
                }
                if child.truncated {
                    rows.push(notice_row(
                        "More entries hidden by safety cap",
                        depth.saturating_add(1),
                    ));
                }
            }
            if rows.len() >= FILE_TREE_MAX_TOTAL_ROWS {
                rows.push(notice_row(
                    "Tree view capped; collapse folders to continue",
                    0,
                ));
                return;
            }
        }
    }
}

fn notice_row(label: &str, depth: u16) -> VisibleRow {
    VisibleRow {
        relative: PathBuf::new(),
        label: label.into(),
        depth,
        kind: FileTreeEntryKind::Notice,
        expanded: false,
        selected: false,
        loading: false,
        size_bytes: None,
    }
}

fn file_tree_colors(colors: EditorColors) -> FileTreeColors {
    FileTreeColors {
        bg: colors.find_bar_bg,
        fg: colors.fg,
        muted: colors.line_number,
        folder_fg: colors.line_number_active,
        selected_bg: colors.selection,
        separator: colors.indent_guide,
    }
}
