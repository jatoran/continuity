# File I/O

Open, save, save-as, drag-drop import, bounded directory listing, external-change detection. A dedicated worker thread serialises disk operations; the UI thread never blocks on filesystem I/O. External edits surface as a non-modal banner with reload / keep-mine / diff actions.

## What it is
- A dedicated thread that handles interactive file reads, writes, shallow folder listings, drag-drop, and external-change watching. Never on the UI thread, ever. UI talks to it via `FileIoClient` (`Sender<FileIoRequest>`) and drains `FileIoEvent` on a 250 ms `WM_TIMER`. File-watching uses `notify` so external edits show up as a non-blocking banner.
- Process-startup paths (`continuity.exe <path>`, Windows "Open with") are partitioned before any window thread spawns. Files are read synchronously and installed into the first restored window as file-associated tabs; folders are forwarded as file-tree roots. This avoids multi-window `FileIoEvent` receiver races and keeps session restore intact.

## Key concepts
- **`FileIoClient`** — clonable `Sender` into the file-I/O thread.
- **`FileIoRequest`** — `OpenFiles | ListDirectory | SaveBuffer | ReloadBuffer | RecheckFile | WatchFile | Shutdown`.
- **`FileIoEvent`** — `Opened | DirectoryListed | Saved | SaveConflict | Reloaded | Rechecked | ExternalChanged | Deleted | EncodingNotice | Failed`.
- **`StartupOpenedFile`** — sync startup-read result: decoded content + `FileAssociation` + optional encoding notice.
- **`FileAssociation { path, mtime_ms, hash, content_hash }`** — link between a buffer and a real file on disk; `hash` fingerprints raw file bytes, `content_hash` fingerprints decoded rope text.
- **`FileBanner`** — non-blocking status banner painted below the tab ribbon. Transient (auto-dismiss) for confirm/info text (save / reload); sticky for decision-required prompts (external-change, deletion, encoding, failures, recovery). See [Banner placement and lifetime](#banner-placement-and-lifetime).
- **`DirectoryEntry`** — one bounded child entry for the file-tree pane; carries relative path, display name, kind, optional size.

## Operations

### Open / import
1. UI dispatches `file.open` → native `IFileOpenDialog` (Phase D9). User picks a file.
2. UI sends `FileIoRequest::OpenFiles { paths, target_pane }`.
3. File-I/O thread reads the file, decodes UTF-8 / UTF-16 BOMs / lossy non-UTF-8 fallback, computes mtime + raw-byte hash + decoded-content hash.
4. File-I/O sends `FileIoEvent::Opened { target_pane, content, file }`.
5. UI tick drains the event and forwards it to the registry as `RegistryEvent::OpenFileBuffer`. The registry resolves the path to one buffer (reusing the existing file buffer when already loaded — one file never spans two edit logs) and either **reveals** the existing tab in its owning window or **spawns** a fresh window, reconciling against the disk bytes in both cases (see [Reconciliation](#reconciliation)). Without a registry (tests) the window creates the buffer locally.
6. For a new buffer, core creates it at the adopted revision with the `FileAssociation` attached; persistence writes an initial snapshot — not an edit.

Drag-drop: `WM_DROPFILES` path. `DragQueryFileW` enumerates dropped paths; image paths route to image import first, files go through the same `OpenFiles` flow, and the first folder opens the file-tree pane.

### Folder open / file tree
1. UI dispatches `file.open_folder` or receives a folder via startup argv, `file.open`, or drag-drop.
2. `Window::open_folder_root` canonicalizes the root, opens the left file-tree pane, and requests the root listing.
3. UI sends `FileIoRequest::ListDirectory { root, relative }` for one directory at a time.
4. Worker calls `file_io_directory::read_directory`, canonicalizes target under root, skips symlinks/artifacts, sorts dirs first, and caps returned entries.
5. Worker sends `FileIoEvent::DirectoryListed { root, relative, entries, truncated }`.
6. UI installs the bounded children into `FileTreeState`; file clicks route back through `file_open_paths_impl`.

See [File tree](file-tree.md) for directory caps, artifact skip list, row caps, and direct-open file-size guard.

### Startup / Open With
1. `app::main` parses `std::env::args_os().skip(1)` as startup paths after the e2e hook, partitioning existing directories from files.
2. `main_initial_requests::build_initial_requests` still builds the normal restored-window `SpawnRequest` list first.
3. `attach_startup_open_files` dedupes argv paths against file-associated buffers already present in the restored pane-tree JSON.
4. For each non-duplicate path, `ui::file_io::read_startup_file` reuses the normal decode/fingerprint primitive synchronously on the app thread.
5. App calls `EditorHandle::open_file_buffer(content, file)` so core owns the new file-associated `Buffer` before window threads start.
6. The first `SpawnRequest` carries `startup_open_buffer_ids`; `Window::new` calls `adopt_startup_open_buffers` after placement replay, adding each id as a tab in the focused pane and saving the updated window state.

Rules:
- Startup opens are additive: restored windows/tabs stay intact; argv files open only in the first restored/new window.
- Startup folders are additive: the first restored/new window opens the first folder in the file-tree pane.
- Duplicate paths are skipped when the same canonical file path is already restored or appears earlier in the same argv list.
- Startup paths suppress first-launch tutorial auto-open so the requested file stays active.
- Full Windows ProgID/default-app registration and single-instance handoff are release-engineering work; current behavior handles only the launched process's argv.

### Save
1. UI dispatches `file.save` → `Window::file_save_impl`.
2. If `editor.trim_trailing_whitespace_on_save` (B14), fire `SelectionEdit::TrimTrailingWhitespaceAll` as one undo group.
3. UI snapshots the rope, sends `FileIoRequest::SaveBuffer { buffer_id, path, content, expected_hash }` where `expected_hash = Some(FileAssociation.hash)` (the last on-disk raw hash this buffer synced to). Save-as / "keep mine" pass `None` to force an unconditional write.
4. **Conflict guard.** When `expected_hash` is `Some`, the worker re-reads the file first; if its current raw hash differs (the file changed externally since we synced), it **refuses the write** and emits `FileIoEvent::SaveConflict { buffer_id, path, content, file }` carrying the disk bytes. This closes the race where a save beats the asynchronous `notify` watcher and silently clobbers an external edit. A missing file is not a conflict (the write recreates it).
5. Otherwise File-I/O writes atomically (temp file + rename), updates mtime + raw-byte hash + decoded-content hash, and sends `FileIoEvent::Saved { buffer_id, file }`.
6. On `Saved`, UI updates the `FileAssociation` (via `SetFileAssociation`) and shows a transient `FileBanner::transient("Saved <path>")`. On `SaveConflict`, UI rolls back the optimistic `mark_saved_clean` (so the buffer is dirty again) and runs reconciliation, which raises the reload / keep-mine / show-diff banner — here "keep mine" force-writes the editor's version (`from_save`), since the user was actively trying to persist.

`file.save_as` (`window_file.rs::file_save_as_impl`) opens the `GetSaveFileNameW` common dialog via `window_file_dialogs.rs::save_file_dialog`; on commit the buffer becomes file-associated and the regular save path runs. `Ctrl+S` on an ephemeral buffer falls through to `save_as` (Phase D8).

#### Save-dialog defaults (Markdown-first)
- **`lpstrDefExt = "md"`** — a name typed without an extension is saved as `.md`. An explicit extension (`notes.txt`) is respected verbatim.
- **Save-specific type filter** (`save_file_dialog::wide_save_filter`) lists `Markdown (*.md, *.markdown)` first with `nFilterIndex = 1`, so the dialog opens defaulting to the Markdown type. Text and All-files remain selectable. (This is a distinct filter from the open-dialog `wide_filter`.)
- **Untitled-buffer seed** — for a buffer with no `FileAssociation`, the file-name box is seeded with the active tab's title (`file_save_as_impl` passes `tab_label` as `default_title`) sanitized to a legal Windows filename stem by `save_file_dialog::sanitize_filename_stem` (strips the pin-dot prefix / trailing ellipsis, swaps reserved chars `\ / : * ? " < > |` for spaces, drops control chars, trims trailing dots/spaces, caps at `MAX_DEFAULT_STEM_CHARS = 48`, falls back to `"untitled"`). The common dialog pre-selects the seeded name so typing replaces it.

### External change watcher
1. UI calls `FileIoClient::watch_file(buffer_id, file)` when a buffer becomes file-associated.
2. File-I/O thread registers a `notify` watch.
3. On change, file-I/O re-reads the file, compares raw-byte hash + mtime against the stored association.
4. If different, emits `FileIoEvent::ExternalChanged { buffer_id, path, content, file }`.
5. UI routes it through `Window::reconcile_file_buffer` (see [Reconciliation](#reconciliation)) — a clean buffer auto-reverts silently, a dirty buffer raises `FileBanner::external(path)` with reload / keep mine / show diff.

### Reconciliation

A file-associated buffer mirrors bytes on disk that other tools can mutate. **Four triggers** expose divergence, and all of them funnel through one decision in `Window::reconcile_file_buffer` (`window_file_reconcile.rs`) so behavior is identical everywhere:

1. **Live watcher** fires while the file is open (`FileIoEvent::ExternalChanged`).
2. **Reopen** of an already-loaded path — the registry resolves it to the existing buffer and **reveals** the tab in its owning window (`WindowControl::RevealBufferTab` → `Window::reveal_file_buffer_tab`) instead of spawning a duplicate, or spawns a window carrying `reconcile_on_init` when no live window owns it.
3. **Session restore** — restored file tabs never read disk, so `Window::recheck_restored_file_buffers` issues a one-shot `FileIoRequest::RecheckFile` at launch; the worker replies `FileIoEvent::Rechecked`.
4. **Save** — the worker's conflict guard refuses a write whose `expected_hash` no longer matches disk and replies `FileIoEvent::SaveConflict` (synchronous, race-free — see [Save](#save)); `Window::reconcile_after_save_conflict` rolls back the optimistic clean state and reconciles. This is the guarantee against silent data loss; the watcher (trigger 1) is the live-notification nicety, not the safety net.

The decision (pure `classify_reconcile`, unit-tested):

| | Disk unchanged | Disk changed externally |
|---|---|---|
| **Buffer clean** (no unexported edits) | reuse as-is | **silent reload** (caret-line anchored) + transient "Reloaded … (changed on disk)" |
| **Buffer dirty** (unexported edits) | reuse as-is | **reload / keep mine / show diff** banner |

"Clean vs. dirty" is `window_paint_builders::is_buffer_dirty_against_file` (rope content hash vs. `FileAssociation.content_hash`). Silent reload is gated by `editor.auto_revert_unmodified` (default on); with it off, a clean external change banners like the dirty case. This fixes the long-standing bug where reopening (or restoring) an externally-changed file showed the stale database copy — and the data-loss footgun where a later save silently clobbered the external edit.

Cross-window ownership lives in the registry's `buffer_home` map (`crates/app/src/registry.rs` + `registry_open_file.rs`), seeded at spawn and pruned on window close.

### Reload
- The conflict banner renders **clickable buttons** (Reload / Keep mine / Show diff) below the message — the geometry is shared between paint and the mouse hit-test (`window_file_banner_buttons.rs`: `compute_banner_geometry` + `Window::try_file_banner_left_down`, claimed in `on_left_button_down` before the editor body). The commands also stay registered (`file.reload_external` / `file.keep_mine` / `file.show_diff`) and self-guard to a no-op when no conflict banner is active.
- User clicks "Reload" → UI sends `FileIoRequest::ReloadBuffer` for that path; on `FileIoEvent::Reloaded`, the UI replaces the buffer content through a whole-buffer edit and refreshes the file association.
- **Show diff** (`window_file_banner_actions.rs::file_show_diff_impl`) opens a scratch read-only tab with a unified line diff (`file_diff.rs`, dependency-free LCS) of editor-buffer vs. on-disk content, instead of a one-line summary. The conflict banner stays sticky so reload / keep-mine still apply.

### Banner placement and lifetime

`FileBanner` (`window_file.rs`) paints a passive panel via `Window::file_banner_overlay`. Two orthogonal axes:

**Placement** — the panel sits *below* the tab ribbon, offset by `TAB_STRIP_HEIGHT_DIP` (`pane_layout::metrics`, 28 DIP) plus a 6 DIP gap, so it never overlaps the tab strip. The text field is `FIELD_HEIGHT_DIP = 26` so descenders clear the `paint_focus_field` inset (no clipping).

**Lifetime** — `expires_at_ms: Option<u64>` distinguishes transient from sticky:
- **Transient** (`FileBanner::transient`, default `TRANSIENT_BANNER_MS = 2500`) — confirm/info banners whose action already completed auto-dismiss. Used for `Saved …` and `Reloaded …`. `is_expired(now_ms)` gates removal; `has_deadline()` keeps the file-I/O poll timer alive until the deadline.
- **Sticky** (`FileBanner::new` / `FileBanner::external*`, `expires_at_ms == None`) — decision-required banners stay until user action (Esc / reload / keep-mine / show-diff). Used for external-change, external-deletion, encoding-notice, write/open/reload/list `Failed`, and recovery prompts.

`transient_for(text, now_ms, duration_ms)` is the caller-duration variant reused by the dirty-tab close arm below.

### Dirty-tab close confirmation (Ctrl+W)

Closing a tab whose buffer is dirty raises a transient `FileBanner`
asking the user to press `Ctrl+W` again to commit. Second press
within `3000 ms`, targeting the same `(PaneId, BufferId)`, performs
the close. The arm state lives on `Window::unsaved_close_arm`
(`UnsavedCloseArm { pane_id, buffer_id, armed_at_ms }`) and reuses
the existing transient-banner surface
(`FileBanner::transient_for(text, now_ms, duration_ms)`) — no new
overlay or modal.

Cancel triggers (clear the arm without closing):
- any dispatched command other than `tab.close`
- editor-body left-click or double-click
- normal tab-strip activation
- focused-pane change
- app focus loss (`WM_ACTIVATEAPP(false)`)
- mouse wheel
- clean close on the same arm's target

Clean-buffer close clears any stale arm and proceeds without showing
the banner. File-associated buffers are dirty only after the rope
revision moves past the import snapshot and the current rope-content
hash differs from `FileAssociation.content_hash`; a freshly opened
file at revision 0 is clean even when decoding stripped a BOM or
normalized UTF-16 / lossy bytes. Commit clears the arm and only clears
the confirmation banner when the active banner text is still the
close-confirmation banner. Top-level `WM_CLOSE` / Alt+F4 still uses
the existing `confirm_close_window` modal; the arm covers `Ctrl+W` tab
close only.

### Trash
- `tab.close` on a non-file-associated buffer sends the buffer to trash (sets `buffers.deleted_at`, adds a `trash` row with expiry).
- `tab.close` on a file-associated buffer just closes the tab — the file on disk is untouched.
- Trash retention defaults to 30 days (`backup.trash_retention_days`); see [persistence](persistence.md).

## Encoding handling
- Read: detect BOM (`UTF-8 BOM`, `UTF-16 BE BOM`, `UTF-16 LE BOM`); fallback to UTF-8 (replacement on decode error) with a banner indicating substitution.
- Write: export the rope as UTF-8 bytes. Non-UTF-8 imports warn because re-save discards the original encoding.
- Reload-with-encoding (Phase C2): click the status-bar encoding chip to reopen with a different decoder.

## File-association integrity
- `mtime_ms` + `hash` store the last observed raw on-disk bytes. Watcher self-write suppression compares both fields so a later same-content touch is still observable.
- `content_hash` stores the decoded rope text at open/save/reload time. Dirty-tab close compares rope bytes against this value, not the raw file hash.
- Startup dedupe compares canonical filesystem paths before import. Hash still belongs to external-change detection, not identity.

## API surface
- `crates/ui/src/file_io.rs::FileIoClient::{open_files, list_directory, save_buffer, reload_buffer, watch_file, events}`.
- `crates/ui/src/file_io.rs::read_startup_file(path) -> StartupOpenedFile`.
- `crates/ui/src/file_io.rs::FileIoEvent` (UI consumer).
- Window-side `file.open` / `file.save` / `file.save_as` handlers in `crates/ui/src/window_file.rs`.
- Window-side folder/file-tree integration in `crates/ui/src/window_file_tree.rs`.

## δ.3 watcher events

The `FileIoEvent` enum carries four post-launch banner-relevant variants in addition to `Opened` / `Saved` / `Reloaded`:

- **`ExternalChanged`** — modified externally; bytes differ from our last self-write fingerprint. UI raises the reload / keep-mine / show-diff banner.
- **`Deleted { buffer_id, path }`** — read fails AND the path is gone from disk. Watcher prunes the `watched` entry; UI raises sticky `FileBanner` "<path> was deleted externally — buffer kept in memory. Save to recreate." The rope is canonical, so the buffer keeps editing; a follow-up `file.save` recreates the file and re-installs the watch.
- **`EncodingNotice { path, encoding }`** — sniffed at open / reload / external-change time when on-disk bytes aren't clean UTF-8. Fires *in addition to* the corresponding `Opened` / `Reloaded` / `ExternalChanged` event (never instead of). UI raises a sticky banner so the user knows re-saving would lose the original encoding (continuity always writes UTF-8).
- **`Failed { operation, path, reason }`** — operation-named banner; the only variant that includes a specific verb (open / save / reload / watch / list folder).
- **`Rechecked { buffer_id, content, file }`** — reply to a `RecheckFile` request (session restore / explicit refresh). Carries the current disk bytes + fingerprint and (re)arms the watch; unlike `ExternalChanged` the worker does *not* gate it on a self-write check — the window owns the clean/dirty decision via `reconcile_file_buffer`. A missing file is silent (no banner; the rope is canonical).

### Encoding sniff contract

`file_io.rs::decode_file_bytes` is the single decode pass; every read path funnels through it via `read_file`. Order:

1. **UTF-8 BOM** (`EF BB BF`) — stripped silently. Still UTF-8; no notice.
2. **UTF-16 LE BOM** (`FF FE`) — decoded with `String::from_utf16_lossy` (invalid surrogates → U+FFFD); notice `"UTF-16 LE"`.
3. **UTF-16 BE BOM** (`FE FF`) — same, notice `"UTF-16 BE"`.
4. **Other invalid UTF-8** — falls back to `String::from_utf8_lossy`; notice `"non-UTF-8"`.

The sniff is conservative on purpose. We don't try to fingerprint Latin-1 / Windows-1252 / Shift-JIS — that would require a real encoding-detection library and the failure mode is silent corruption. The contract: if bytes don't decode cleanly as UTF-8, the user sees a banner and U+FFFD replacements; the user decides whether to save (overwriting the original encoding) or close without saving.

## Configuration
- `editor.trim_trailing_whitespace_on_save` (B14, default `true`).
- `editor.auto_revert_unmodified` (default `true`) — silently reload a clean file-associated buffer when its file changes on disk; a dirty buffer always banners. Mirrored onto `ViewOptions::auto_revert_unmodified`.
- `file.default_encoding` (default `"utf-8"`).
- `file.watch_external_changes` (default `true`).

## Key files
- startup argv intake: `crates/app/src/main.rs`
- launch request restore: `crates/app/src/main_initial_requests.rs`
- startup tab adoption: `crates/ui/src/window_startup_open.rs`
- file-I/O thread + worker: `crates/ui/src/file_io.rs`
- directory listing caps + root containment: `crates/ui/src/file_io_directory.rs`
- file-I/O worker loop: `crates/ui/src/file_io_worker.rs`
- Window save/open: `crates/ui/src/window_file.rs`
- File tree: `crates/ui/src/file_tree.rs`, `crates/ui/src/window_file_tree.rs`
- file dialog wrappers: `crates/ui/src/window_file_dialogs.rs::{open_file_dialog, open_folder_dialog, save_file_dialog}`
- DragDrop receiver: `crates/ui/src/window.rs` (`WM_DROPFILES` arm) + `crates/ui/src/window_file.rs::on_drop_files`
- file association on Buffer: `crates/buffer/src/metadata.rs`
- dirty-tab close arm: `crates/ui/src/window_close_confirm.rs`
- external-change reconciliation: `crates/ui/src/window_file_reconcile.rs` (decision) + `window_file_recheck.rs` (restore recheck) + `window_reveal.rs` (cross-window reveal)
- conflict-banner actions + diff view: `crates/ui/src/window_file_banner_actions.rs` + `crates/ui/src/file_diff.rs`
- cross-window open routing: `crates/app/src/registry_open_file.rs` (+ `registry.rs` `buffer_home`)

## Relates to
- [Buffer](buffer.md) — `FileAssociation` lives on the buffer; updated via `SetFileAssociation`.
- [Persistence](persistence.md) — adopting a file writes an initial snapshot; saved bytes land on disk only via this thread.
- [Settings](settings.md) — `[file]` and `[editor]` blocks carry the relevant toggles.
- [Overlays](overlays.md) — `FileBanner` is a non-blocking banner (not an overlay; doesn't preempt input).
- [File tree](file-tree.md) — folder browsing surface built on `ListDirectory` events.
