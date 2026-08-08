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

mod rename;
pub(crate) use rename::FileTreeRenameState;

const FILE_TREE_MAX_TOTAL_ROWS: usize = 50_000;
const FILE_TREE_PAINT_OVERSCAN_ROWS: usize = 4;

/// Maximum file size opened directly from the tree.
pub(crate) const FILE_TREE_MAX_OPEN_BYTES: u64 = 8 * 1024 * 1024;

/// Window-owned file-tree state.
#[derive(Debug)]
pub(crate) struct FileTreeState {
    root: Option<PathBuf>,
    visible: bool,
    nodes: HashMap<PathBuf, FileTreeNode>,
    pending: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    scroll_offset_dip: f32,
    viewport_height_dip: f32,
    hit_rows: Vec<FileTreeHitRow>,
    width_dip: f32,
    expanded_directories: HashSet<PathBuf>,
    keyboard_focused: bool,
    rename: Option<FileTreeRenameState>,
}

impl Default for FileTreeState {
    fn default() -> Self {
        Self {
            root: None,
            visible: false,
            nodes: HashMap::new(),
            pending: HashSet::new(),
            selected: None,
            scroll_offset_dip: 0.0,
            viewport_height_dip: 0.0,
            hit_rows: Vec::new(),
            width_dip: FILE_TREE_DEFAULT_WIDTH_DIP,
            expanded_directories: HashSet::new(),
            keyboard_focused: false,
            rename: None,
        }
    }
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
            self.width_dip
        } else {
            0.0
        }
    }

    pub(crate) fn width_dip(&self) -> f32 {
        self.width_dip
    }

    pub(crate) fn set_width_dip(&mut self, width_dip: f32) {
        self.width_dip = width_dip.clamp(140.0, 720.0);
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

    pub(crate) fn open_root(
        &mut self,
        root: PathBuf,
        workspace: Option<&continuity_config::VaultWorkspaceState>,
    ) -> PathBuf {
        self.root = Some(root.clone());
        self.visible = workspace.is_none_or(|state| state.file_tree_visible);
        if let Some(state) = workspace {
            self.width_dip = state.file_tree_width_dip.clamp(140.0, 720.0);
        }
        self.expanded_directories = workspace
            .map(|state| {
                state
                    .expanded_directories
                    .iter()
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
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
    ) -> Option<Vec<PathBuf>> {
        if self.root.as_deref() != Some(root) {
            return None;
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
            let expanded = kind == FileTreeNodeKind::Directory
                && self.expanded_directories.contains(&entry.relative);
            child_nodes.push((
                entry.relative.clone(),
                FileTreeNode {
                    name: entry.name,
                    kind,
                    relative: entry.relative,
                    size_bytes: entry.size_bytes,
                    expanded,
                    loaded: kind == FileTreeNodeKind::File,
                    truncated: false,
                    children: Vec::new(),
                },
            ));
        }
        let parent = self.nodes.get_mut(&relative)?;
        parent.loaded = true;
        parent.truncated = truncated;
        parent.children = children;
        let mut directories_to_load = Vec::new();
        for (relative, node) in child_nodes {
            if self.visible
                && node.kind == FileTreeNodeKind::Directory
                && node.expanded
                && !node.loaded
            {
                directories_to_load.push(relative.clone());
            }
            self.nodes.insert(relative, node);
        }
        Some(directories_to_load)
    }

    pub(crate) fn toggle_directory(&mut self, relative: &Path) -> Option<PathBuf> {
        let node = self.nodes.get_mut(relative)?;
        if node.kind != FileTreeNodeKind::Directory {
            return None;
        }
        node.expanded = !node.expanded;
        if node.expanded {
            self.expanded_directories.insert(node.relative.clone());
        } else {
            self.expanded_directories.remove(&node.relative);
        }
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

    /// Set or clear the highlighted row. Used to mirror the focused tab's
    /// file, where `None` means the focused buffer is not represented in
    /// the tree.
    pub(crate) fn set_selected(&mut self, relative: Option<PathBuf>) {
        self.selected = relative;
    }

    pub(crate) fn expanded_directories_needing_load(&self) -> Vec<PathBuf> {
        self.nodes
            .values()
            .filter(|node| {
                node.kind == FileTreeNodeKind::Directory
                    && node.expanded
                    && !node.loaded
                    && !self.pending.contains(&node.relative)
            })
            .map(|node| node.relative.clone())
            .collect()
    }

    pub(crate) fn vault_workspace_state(&self) -> continuity_config::VaultWorkspaceState {
        let mut expanded_directories: Vec<_> = self
            .expanded_directories
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect();
        expanded_directories.sort_unstable();
        continuity_config::VaultWorkspaceState {
            file_tree_width_dip: self.width_dip,
            file_tree_visible: self.visible,
            expanded_directories,
            ..continuity_config::VaultWorkspaceState::default()
        }
    }

    pub(crate) fn reassociate_expanded_path(&mut self, source: &Path, destination: &Path) -> bool {
        if let Some(selected) = self.selected.as_ref() {
            if let Ok(suffix) = selected.strip_prefix(source) {
                self.selected = Some(destination.join(suffix));
            }
        }
        if self
            .rename
            .as_ref()
            .is_some_and(|rename| rename.relative().starts_with(source))
        {
            self.rename = None;
        }
        let replacements: Vec<_> = self
            .expanded_directories
            .iter()
            .filter_map(|path| {
                path.strip_prefix(source)
                    .ok()
                    .map(|suffix| (path.clone(), destination.join(suffix)))
            })
            .collect();
        if replacements.is_empty() {
            return false;
        }
        for (source, destination) in replacements {
            self.expanded_directories.remove(&source);
            self.expanded_directories.insert(destination);
        }
        true
    }

    pub(crate) fn remove_expanded_path(&mut self, deleted: &Path) -> bool {
        let before = self.expanded_directories.len();
        self.expanded_directories
            .retain(|path| !path.starts_with(deleted));
        self.expanded_directories.len() != before
    }

    pub(crate) fn absolute_path(&self, relative: &Path) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join(relative))
    }

    pub(crate) fn row_at(&self, x: f32, y: f32) -> Option<FileTreeHitRow> {
        let row = self.visible_row_slot_at(x, y)?;
        self.hit_rows.get(row).cloned()
    }

    pub(crate) fn drop_target_top_at(&self, x: f32, y: f32) -> Option<f32> {
        let row = self.visible_row_slot_at(x, y)?;
        let first = (self.scroll_offset_dip / FILE_TREE_ROW_HEIGHT_DIP).floor();
        let first_top =
            FILE_TREE_HEADER_HEIGHT_DIP + first * FILE_TREE_ROW_HEIGHT_DIP - self.scroll_offset_dip;
        Some(first_top + row as f32 * FILE_TREE_ROW_HEIGHT_DIP)
    }

    fn visible_row_slot_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.visible || !(0.0..self.width_dip).contains(&x) {
            return None;
        }
        if y < FILE_TREE_HEADER_HEIGHT_DIP || y >= self.viewport_height_dip {
            return None;
        }
        let first = (self.scroll_offset_dip / FILE_TREE_ROW_HEIGHT_DIP).floor();
        let first_top =
            FILE_TREE_HEADER_HEIGHT_DIP + first * FILE_TREE_ROW_HEIGHT_DIP - self.scroll_offset_dip;
        let row = ((y - first_top) / FILE_TREE_ROW_HEIGHT_DIP).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        (row < self.hit_rows.len()).then_some(row)
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
        vault_config: Option<&continuity_config::VaultConfig>,
    ) -> Option<FileTreeDraw> {
        if !self.visible {
            return None;
        }
        self.viewport_height_dip = client_height_dip;
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
                color_override: vault_row_color(vault_config, &row.relative, row.kind),
                inline_edit: self
                    .rename
                    .as_ref()
                    .filter(|rename| rename.relative() == row.relative)
                    .map(FileTreeRenameState::build_draw),
            });
        }
        let title = self
            .root
            .as_deref()
            .map(project_name)
            .unwrap_or_else(|| "No folder".into());
        Some(FileTreeDraw {
            rect: (0.0, 0.0, self.width_dip, client_height_dip),
            title,
            rows: draw_rows,
            colors: file_tree_colors(colors),
            first_row_index: first as u32,
            row_height_dip: FILE_TREE_ROW_HEIGHT_DIP,
            header_height_dip: FILE_TREE_HEADER_HEIGHT_DIP,
            scroll_offset_dip: self.scroll_offset_dip,
            content_height_dip: content_height,
            drag: None,
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

fn project_name(root: &Path) -> String {
    root.components()
        .next_back()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| root.display().to_string())
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

fn vault_row_color(
    config: Option<&continuity_config::VaultConfig>,
    relative: &Path,
    kind: FileTreeEntryKind,
) -> Option<continuity_render::Rgba> {
    let config = config?;
    let is_directory = kind == FileTreeEntryKind::Directory;
    let mut value = if is_directory {
        &config.appearance.folder_color
    } else {
        &config.appearance.file_color
    };
    for style in &config.files.styles {
        if style.matches(relative, is_directory) {
            value = &style.color;
        }
    }
    let color: continuity_theme::Color = value.parse().ok()?;
    Some(continuity_render::Rgba {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: color.a as f32 / 255.0,
    })
}

#[cfg(test)]
#[path = "file_tree/tests.rs"]
mod tests;
