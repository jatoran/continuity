# Vaults

A vault is an opened folder with a `.continuity/vault.toml` marker. It keeps the ordinary file-tree workflow, adds folder-scoped continuous export, and supplies portable tree and appearance policy. Opening a folder without the marker remains ordinary manual-save browsing.

## Activation and scope

- `file.open_folder` asks the file-I/O worker to canonicalize the selected folder and walk its ancestors for the nearest `.continuity/vault.toml`.
- The nearest marker wins, so a nested vault can override an outer vault. A `.continuity` directory without `vault.toml` is not a vault.
- A normal folder shows a non-modal **Initialize vault** banner action. Initialization creates the marker with validated defaults and never overwrites an existing file.
- Vault ownership is per window and UI-thread owned. Files outside the active root, and files matched by vault ignore rules, retain normal manual-save behavior.
- The opened root remains in the window workspace JSON. Vault-specific file-tree width, visibility, and expanded folders live in `.continuity/workspace.toml` and override generic window chrome state when that vault opens. An explicit startup folder takes precedence over a restored root.

## Continuous export

The database and rope remain canonical. Vault autosave is continuous export of file-associated buffers under the active root:

1. An edit schedules the buffer for the configured idle delay; the 100 ms file-I/O tick also discovers dirty vault buffers so uncommon mutation paths cannot miss scheduling.
2. The default delay is 750 ms. Focus loss, pane/tab switches, and window shutdown force pending exports to the worker.
3. The worker writes with the buffer's last raw-byte hash as `expected_hash`. If disk changed, the write is refused and the normal reload / keep mine / show diff banner handles the conflict.
4. A conflicted buffer is removed from the autosave queue until the user decides, preventing retry loops. Reload adopts disk; Keep Mine is an explicit force-export in a vault.
5. Autosave does not trim or otherwise rewrite text, and successful automatic exports do not show the normal “Saved” banner or status notice.
6. Moving an open vault entry updates its `FileAssociation`, registry path index, and file watch. Recycling an open entry detaches its association and cancels pending exports so it is not recreated.

## File-tree behavior

- Plain click replaces the focused tab with the selected file.
- Ctrl+click and middle-click both open a permanent focused tab in the same window.
- The file-tree highlight tracks the focused tab's file, and clears when the focused buffer has no file or lives outside the root.
- Shift+click opens a new top-level window and carries the vault root so the new window has the same file sidebar.
- Markdown extensions are hidden case-insensitively when enabled. If hiding `.md` would collide with another displayed name, the extension remains visible.
- Right-click creates a uniquely named `Untitled.md` or `New Folder`, or moves an entry to the Windows Recycle Bin after a second confirmation within three seconds.
- Dragging an entry onto a folder moves it there; dragging onto a file uses that file's parent. `.continuity`, root escapes, symlinks, self-nesting folders, and destination collisions are rejected by the worker.
- Drag moves show a source ghost and hovered-row drop highlight before commit.
- `F2` or the entry context menu starts inline rename. The editor exposes the actual filename even when `.md` is hidden, selects the stem for files and the whole name for folders, and commits through the contained file-I/O worker. Open buffer associations, watches, and portable expanded-folder paths follow successful renames.
- The sidebar is drag-resizable from 140–720 DIP with a resize cursor. It stops above the status bar. A purpose-drawn pane/sidebar action at bottom-left collapses it, with a smaller settings-controls action immediately to its right. Purpose-drawn outline-list and miniature-document-map actions occupy fixed slots at bottom-right. All action strokes use the normal status foreground, and every actionable status segment uses the standard pointer cursor on hover.
- Vault-launcher shortcut creation writes a native desktop `.lnk` whose filename is the sanitized vault name alone (for example, `Code Vault.lnk`), adding ` (2)`, ` (3)`, and so on only for collisions. The shortcut launches `continuity.exe --vault <root>`.

## Configuration

The marker is TOML version 1:

Choose **Open Vault Settings** from the tree context menu or click the compact settings-controls action beside the file-tree action to edit the hidden marker in Continuity. The marker remains excluded from vault content autosave; save it explicitly, after which the watcher validates and applies the new settings.

```toml
version = 1

[save]
autosave = true
delay_ms = 750 # validated 100..=60000

[files]
hide_markdown_extensions = true
folders_first = true
sort = "name" # name | modified | created
descending = false
ignore = [
    ".trash",
    # "*.tmp",
    # "archive/**",
    # "!archive/keep.md",
]

# Optional path-specific colors; the last matching rule wins.
# [[files.styles]]
# pattern = "daily/**"
# kind = "file" # any | file | folder
# color = "#b58900"

[appearance]
theme = "solarized_dark"
file_color = "#839496"
folder_color = "#268bd2"

# Optional stable theme-token overrides.
# [appearance.colors]
# "editor.caret_line_highlight" = "#073642"
```

Ignore and style patterns are relative, slash-normalized, case-insensitive wildcard rules; `*` and `?` are supported, ordered ignore rules use leading `!` to re-include, and `.continuity` is always hidden. Appearance overrides can replace only keys already present in the resolved theme. Precedence is global theme → vault base theme → vault token overrides; a later runtime theme choice remains a runtime choice until settings or vault configuration reloads.

Continuity owns a second version-1 file, `.continuity/workspace.toml`, for portable UI state:

```toml
version = 1
file_tree_width_dip = 280.0
file_tree_visible = true
expanded_directories = ["notes", "projects/continuity"]
```

The UI thread snapshots state after completed resize and tree visibility/expansion changes; the file-I/O worker validates and writes it. Paths are relative, slash-normalized, bounded, and cannot escape the vault. Missing state uses defaults. Invalid state does not block vault activation: defaults apply and a non-modal failure banner identifies the file. A collapsed sidebar defers loading restored descendants until expansion.

The worker watches each active marker and fans validated config reloads and in-app tree mutations to every subscribed vault window. Invalid config leaves the last valid state active and raises a non-modal failure banner.

Closing the only tab while any folder tree is open leaves the window in a zero-tab workspace so the tree remains usable. A second `Ctrl+W` closes that window. The UI uses an empty synthetic read-only render-backing buffer for this transient state, so typing cannot mutate a hidden tab, and installs a clean placeholder only immediately before window persistence, keeping the pane-tree wire invariant intact.

## Ownership and layers

- `config` owns parsing, validation, wildcard policy, vault-workspace shape, and defaults; it performs no filesystem discovery.
- The single `file-io` worker owns discovery, initialization, shallow listing, workspace-state writes, filesystem watches, create/move/rename/recycle operations, and per-window reply routing.
- Each `ui::Window` owns its `VaultState`, autosave schedule, live tree selection/expansion/width, banners, and theme projection.
- `core` remains the only writer of buffer state. `render` receives immutable visible rows and status actions only.
- `app` owns same-window versus new-window routing and passes the vault root into Shift+click window construction.

## Key files

- Config contract: `crates/config/src/vault.rs`
- Workspace-state contract: `crates/config/src/vault_workspace.rs`
- Discovery/init: `crates/ui/src/file_io_vault.rs`
- Contained filesystem mutation: `crates/ui/src/file_io_vault_entries.rs`
- Worker and routed events: `crates/ui/src/file_io_worker.rs`, `crates/ui/src/file_io_vault_client.rs`
- Window state/autosave/theme: `crates/ui/src/vault.rs`, `window_vault_autosave.rs`, `window_vault_theme.rs`
- Workspace I/O/dispatch: `crates/ui/src/file_io_vault_workspace.rs`, `window_vault_workspace.rs`
- Tree interaction/resize: `crates/ui/src/window_file_tree.rs`, `window_file_tree_resize.rs`
- App open routing: `crates/app/src/registry_open_file.rs`

## Related

- [File tree](file-tree.md)
- [File I/O](file-io.md)
- [Themes](theme.md)
- [Panes, tabs, and windows](panes-tabs-windows.md)
