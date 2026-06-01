# File Tree

The file tree lets a window expose one opened folder beside the editor without making the filesystem canonical. It is a bounded UI projection: directory reads happen on the file-I/O worker, file clicks route through normal file-open handling, and the pane never edits disk.

## What it is
- Left pane for browsing one opened folder and opening text files from that folder.
- UI state is owned by `ui::Window`; filesystem enumeration is delegated to the file-I/O worker; rendering consumes a per-frame `FileTreeDraw` payload.
- The pane is chrome, not buffer content. It never mutates directories and never changes the canonical rope.

## Key concepts
- **Folder root**: canonical directory chosen by `file.open_folder`, process argv, drag/drop, or folder path passed to `file.open`.
- **Relative path**: every tree node is stored relative to the opened root. Absolute child paths are derived only at open time.
- **Directory listing**: one shallow worker result for one expanded directory.
- **Visible rows**: flattened UI projection clipped to viewport + small overscan before render.
- **Notice row**: non-interactive row for loading/cap messages.

## Data model
- `FileTreeState` owns:
  - `root: Option<PathBuf>`
  - `nodes: HashMap<PathBuf, FileTreeNode>`
  - `pending: HashSet<PathBuf>`
  - `selected: Option<PathBuf>`
  - `scroll_offset_dip`
  - `hit_rows`
- `FileTreeNode` stores kind, name, relative path, loaded/expanded/truncated flags, optional file size, and child relative paths.
- `FileTreeDraw` stores only visible rows, pane rect, header title, row metrics, scroll offset, and colors.

## Operations
- `file.open_folder` with `null` opens a native folder picker; string args open that folder directly.
- `view.toggle_file_tree` toggles visibility and refreshes pane layout.
- `file.open` accepts files and folders. File paths import through existing file-open flow; the first folder path opens the file tree.
- Startup argv paths are partitioned: files become startup tabs, folders open the first restored/new window's file-tree pane.
- `WM_DROPFILES` partitions image drops first, then opens dropped files and the first dropped folder through the same path.
- Directory click toggles expansion; if the node has not been loaded, the UI sends `FileIoRequest::ListDirectory`.
- File click opens via the existing `file_open_paths_impl` path so buffer adoption, file association, watchers, and encoding banners stay consistent.
- Wheel inside the left pane scrolls `FileTreeState`; editor scroll inertia is not touched.
- Right-click inside the visible tree opens a one-item chrome context menu: `Toggle File Tree`. The command dispatches `view.toggle_file_tree`.
- Cursor over the tree is the default arrow, not the text I-beam.
- Paint builds the visible row slice during `Window::on_paint`, then `renderer_post_body` paints the pane below modal overlays and above editor chrome.

## Safety + performance contract
- No recursive directory walk. Each expansion lists one directory only.
- Per-directory return cap: `DIRECTORY_LIST_MAX_ENTRIES = 512`.
- Per-directory scan cap: `DIRECTORY_SCAN_MAX_ENTRIES = 4096`.
- UI flattened-row cap: `FILE_TREE_MAX_TOTAL_ROWS = 50_000`.
- Render path receives visible rows only: viewport rows + `FILE_TREE_PAINT_OVERSCAN_ROWS = 4`.
- Direct file-open cap from the tree: `FILE_TREE_MAX_OPEN_BYTES = 8 MiB`; larger files show a banner and require explicit `file.open`.
- Directory listings reject relative paths with root, prefix, current-dir, or parent components.
- Worker canonicalizes root and target, then rejects targets that do not stay under the opened root.
- Symlinks and non-file/non-directory entries are skipped.
- Common artifact directories are skipped: `.git`, `.hg`, `.svn`, `.cache`, `.mypy_cache`, `.pytest_cache`, `.ruff_cache`, `.next`, `.nuxt`, `.turbo`, `.venv`, `.vs`, `__pycache__`, `build`, `coverage`, `dist`, `node_modules`, `target`, `venv`.
- The feature exposes no delete, rename, move, mkdir, or write operation.

## Rendering
- `FileTreeDraw` is a render-crate payload, not UI state.
- Text paints through one-line `IDWriteTextLayout`s with `DWRITE_WORD_WRAPPING_NO_WRAP` and a per-label clip rect. Long file names clip within their row and never wrap into later rows.
- The pane width is fixed by `FILE_TREE_DEFAULT_WIDTH_DIP = 280.0`; body panes start at `file_tree.visible_width_dip()`.
- Colors derive from `EditorColors`; no separate theme keys exist yet.

## API surface
- Commands:
  - `file.open_folder`
  - `view.toggle_file_tree`
- File-I/O worker:
  - `FileIoRequest::ListDirectory { root, relative }`
  - `FileIoEvent::DirectoryListed { root, relative, entries, truncated }`
- Internal UI:
  - `Window::open_folder_root`
  - `Window::handle_file_tree_directory_list`
  - `Window::try_file_tree_left_down`
  - `Window::try_file_tree_mouse_wheel`
  - `Window::chrome_context_target_at`

## Configuration
- None today. Width, caps, artifact skips, and 8 MiB direct-open limit are code constants.

## Key files
- UI state: `crates/ui/src/file_tree.rs`
- UI command/mouse/event integration: `crates/ui/src/window_file_tree.rs`
- Chrome hit-test: `crates/ui/src/window_right_edge_chrome.rs`
- Context menu dispatch: `crates/ui/src/window_context_menu.rs`
- File command trait impl: `crates/ui/src/window_file_context.rs`
- File-I/O request/event definitions: `crates/ui/src/file_io.rs`
- Directory listing worker helper: `crates/ui/src/file_io_directory.rs`
- File-I/O worker loop: `crates/ui/src/file_io_worker.rs`
- Native folder dialog: `crates/ui/src/window_file_dialogs.rs`
- Pane layout reservation: `crates/ui/src/window_panes.rs`
- Paint payload build: `crates/ui/src/window_paint.rs`
- Render payload: `crates/render/src/file_tree.rs`
- Render painter: `crates/render/src/file_tree_paint.rs`
- Post-body paint stage: `crates/render/src/renderer_post_body.rs`
- Command IDs: `crates/command/src/file.rs`, `crates/command/src/view.rs`
- Command trait split: `crates/command/src/file_context.rs`
- Startup folder routing: `crates/app/src/main.rs`, `crates/app/src/registry.rs`

## Relates to
- [File I/O](file-io.md) — file reads, saves, watches, banners, and directory listing worker.
- [Rendering](rendering.md) — `FileTreeDraw` is painted in the post-body chrome pass.
- [Command system](command-system.md) — command dispatch enters through `FileContext`.
- [Panes, tabs, windows](panes-tabs-windows.md) — pane root rect reserves the left file-tree width.
