# File Tree

The file tree is a bounded left-hand projection of one opened folder. Files remain exports of canonical buffers; the tree stores relative UI state and delegates every filesystem operation to the file-I/O worker.

## Modes

- An ordinary opened folder provides browsing and normal manual-save file behavior.
- A folder at or below the nearest `.continuity/vault.toml` becomes a vault. Vault behavior is specified in [Vaults](vaults.md).
- `Window` owns the root, expanded nodes, selection, pending requests, scroll offset, hit rows, visibility, and width. Generic folder state uses the window workspace codec; vault width, visibility, and expanded folders restore from `.continuity/workspace.toml`.

## Navigation

- `file.open_folder` uses the native folder picker; folder arguments, startup folders, and dropped folders use the same inspection path.
- Directory click expands/collapses a node and requests one shallow listing if needed.
- Plain file click replaces the focused tab. Ctrl+click and middle-click both open a new focused tab. Shift+click opens a top-level window; from a vault it inherits the vault sidebar.
- The tree highlights the focused tab's file: switching tabs by any means moves the highlight, and it clears when the focused buffer has no file or lives outside the tree root.
- File-backed tabs use the filename as their title. Content-derived titles are reserved for ephemeral buffers without a file association.
- The tree heading is the project directory name only (for example `Code Vault 2`), never the canonical/device-prefixed absolute path.
- Files larger than 8 MiB show a banner and require explicit `file.open`.
- Wheel input inside the sidebar scrolls only the tree. The cursor stays the default arrow.
- Clicking a file or folder gives the tree keyboard focus. `F2` or **Rename** in the row context menu opens an inline single-line editor using the real filename (including a hidden `.md` extension). Enter or Tab commits, Escape cancels, and standard selection/clipboard/navigation chords work inside the field.
- Rename is available in ordinary folders and vaults. Names that are empty, reserved by Windows, contain separators/invalid characters, collide with a sibling, escape the root, or target the protected `.continuity` directory are rejected without closing the inline field.

## Vault interactions

- Right-click blank space or an entry to create `Untitled.md` or `New Folder` in the relevant directory.
- Right-click an entry twice within three seconds to move it to the Windows Recycle Bin.
- Drag an entry onto a directory to move it there, or onto a file to move it to that file's parent. The in-flight row is painted beside the pointer and the hovered drop row is highlighted before release.
- **Open Vault Settings** in the tree context menu opens the hidden `.continuity/vault.toml` marker as a permanent tab.
- **Create Desktop Shortcut** writes a collision-safe `.lnk` on the Windows desktop targeting the current executable with `--vault "<root>"` and the app icon.
- The `.continuity` directory is protected and hidden. Worker containment checks reject root escapes, symlinks, folder self-nesting, and collisions.
- Successful rename refreshes the containing listing, follows open buffer associations and file watches, and rewrites expanded-directory paths when a folder moves. Vault expansion changes are persisted to `.continuity/workspace.toml`.
- File/folder label colors, ignore rules, sort order, and `.md` label elision come from the vault config. Elision is collision-safe.

## Layout and status actions

- Default width is 280 DIP; the separator drag range is 140–720 DIP and shows the horizontal-resize cursor throughout its grab band.
- Body panes begin at `file_tree.visible_width_dip()` and the usual caret-line layout anchor handles the resulting reflow.
- The tree stops above the global status strip. An always-visible project-grid action opens the vault launcher. In a vault, a purpose-drawn pane/sidebar action collapses/expands the tree and a smaller settings-controls action sits beside it. Purpose-drawn outline-list and miniature-document-map actions stay at bottom-right. Actions use the normal status foreground and the standard pointer cursor; disabled toggles dim without changing their hit target. Paint and hit-test both use full client width and fixed icon slots independent of font metrics.
- Vault tree changes snapshot to `.continuity/workspace.toml` through the file-I/O worker. Restored expansion remains shallow: each listed expanded directory schedules its own bounded child listing, and a hidden tree defers descendant requests until shown.

## Vault launcher

- `vault.launcher_show` (`Ctrl+K, V`) and the always-visible status action open a compact fuzzy picker backed by SQLite `known_vaults` rows.
- Successful vault activation refreshes recency. Pinned entries sort first; `Alt+P` toggles the selected pin and `Ctrl+Delete` forgets only the history row.
- Enter reuses a matching vault window only when it is on the current virtual desktop; otherwise it creates a window on the current desktop. `Ctrl+Enter` explicitly replaces the current window's folder context.
- Browse and Initialize are mouse/keyboard rows in the launcher. `Alt+S` creates a desktop shortcut for the selected vault.
- Rendering consumes only visible rows in `FileTreeDraw`; one-line DirectWrite labels are clipped to their row and can take per-entry color overrides.

## Safety and bounds

- Directory enumeration is worker-only and never recursive: one expansion lists one directory.
- Return cap: 512 entries; scan cap: 4096 entries; flattened UI cap: 50,000 rows; paint overscan: 4 rows.
- Roots and targets are canonicalized, relative paths reject parent/root/prefix components, and targets must stay under the root.
- Symlinks and non-file/non-directory entries are skipped.
- Common artifact directories such as `.git`, `node_modules`, `target`, build outputs, and virtual environments are skipped in every mode. Vault ignore rules apply afterward.
- Directory/list/mutation completions use the requesting window's reply channel; vault config and in-app mutations fan out to all subscribed windows.

## API and files

- Commands: `file.open_folder`, `view.toggle_file_tree`.
- Requests/events: `InspectFolder`, `InitializeVault`, `ListDirectory`, `CreateVaultEntry`, `MoveVaultEntry`, `DeleteVaultEntry`, `PersistVaultWorkspace`; `FolderInspected`, `DirectoryListed`, `VaultEntriesChanged`, `VaultConfigChanged`.
- UI: `crates/ui/src/file_tree.rs`, `file_tree/rename.rs`, `window_file_tree.rs`, `window_file_tree_rename.rs`, `window_file_tree_resize.rs`, `window_context_menu.rs`.
- Worker: `crates/ui/src/file_io_directory.rs`, `file_io_vault.rs`, `file_io_vault_entries.rs`, `file_io_vault_workspace.rs`, `file_io_worker.rs`.
- Render: `crates/render/src/file_tree.rs`, `file_tree_paint.rs`.

## Related

- [Vaults](vaults.md)
- [File I/O](file-io.md)
- [Rendering](rendering.md)
- [Panes, tabs, and windows](panes-tabs-windows.md)
