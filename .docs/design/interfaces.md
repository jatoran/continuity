# Interfaces

## Scope
- In: cross-thread message types, command dispatch contract, context method surface, top-level event channels.
- Out: per-feature payload shapes (see `features/*`), Win32 message dispatch (see `technical/paint-flow.md`).

## Vocabulary
- **`Command`**: a `CommandId` (`&'static str`) + `ContextPredicate` + `Handler` closure. Registered in `command::Registry`.
- **`Context`**: the trait that handlers operate against. `ui::Window` is the only production impl.
- **`FileContext`**: optional file/folder command surface returned by `Context::file_context()`.
- **`ContextPredicate`**: a small boolean grammar over `Context` atoms (`editor.focused`, `selection.is_caret`, `shift.held`, `language`).
- **`SelectionEdit`**: the typed payload for every buffer mutation. 39+ variants today.
- **`EditorMessage`**: the channel payload UI → core.
- **`EditEvent`**: the channel payload core → subscribers.

## `EditorMessage`

```rs
enum EditorMessage {
    OpenBuffer        { content: String, reply: Sender<BufferId> },
    OpenFileBuffer    { content: String, file: FileAssociation,
                        reply: Sender<BufferId> },
    AdoptBuffer       { buffer: Buffer, next_seq: u64, last_snapshot_at_ms: i64,
                        reply: Sender<BufferId> },
    ApplyEdit         { buffer_id: BufferId, op: EditOp,
                        reply: Sender<Result<Revision, Error>> },
    ApplySelectionEdit{ buffer_id: BufferId, edit: SelectionEdit,
                        reply: Sender<Result<Option<Revision>, Error>> },
    ApplyEditGroup    { buffer_id: BufferId, ops: Vec<EditOp>,
                        selections_after: Vec<Selection>,
                        command_name: &'static str,
                        reply: Sender<Result<Option<Revision>, Error>> },
    SetSelections     { buffer_id: BufferId, selections: Vec<Selection>,
                        reply: Sender<Result<(), Error>> },
    MutateSelections  { buffer_id: BufferId,
                        f: Box<dyn FnOnce(&mut Vec<Selection>) + Send>,
                        reply: Sender<Result<(), Error>> },
    Snapshot          { buffer_id: BufferId, reply: Sender<Option<EditorSnapshot>> },
    SetFileAssociation{ buffer_id: BufferId, file: Option<FileAssociation>,
                        reply: Sender<Result<(), Error>> },
    Undo / Redo / RedoAlt / NamedSnapshot / …                // see `core::message`
    Shutdown,
}
```

Every variant carries a reply `Sender`. UI calls block on the reply — typical round-trip is well under one frame budget.

Single rule: **never call any `EditorHandle` method from inside a `mutate_selections` closure**. That closure is already inside the core thread; re-entering would deadlock.

## `EditEvent`

```rs
enum EditEvent {
    BufferOpened       { id: BufferId },
    EditApplied        { id: BufferId, revision: Revision },
    SelectionsChanged  { id: BufferId },
    FileAssociationChanged { id: BufferId },
    BufferTouched      { id: BufferId },                 // last_touched bumped, no edit
}
```

Subscribers fan-out via `EditorHandle::events() -> &Receiver<EditEvent>`. Slow subscribers do not slow producers; the broadcast layer drops to a stale subscriber.

## Command surface

```rs
struct CommandId(pub &'static str);                       // e.g. "editor.indent"

type Handler = Arc<dyn Fn(&Value, &mut dyn Context)
                          -> Result<(), Error> + Send + Sync>;

impl Registry {
    fn register(&mut self, id: CommandId, when: ContextPredicate, handler: Handler);
    fn register_palette_safe(&mut self, id: CommandId,
                             when: ContextPredicate, handler: Handler);  // §A7
    fn dispatch(&self, id: CommandId, args: &Value, ctx: &mut dyn Context)
        -> Result<(), Error>;
    fn handler_for_name(&self, name: &str, ctx: &dyn Context) -> Result<Handler, Error>;
    fn is_palette_safe(&self, id: &str) -> bool;
    fn palette_safe_ids(&self) -> Vec<&'static str>;
}
```

`palette_safe` (Phase A7) is a metadata flag — slash commands (H5) and future restricted palette modes filter on it.

### `ContextPredicate` grammar

```
expr  := atom (&& atom)*
atom  := 'true' | 'false' | path | path '==' lit | path '!=' lit
path  := IDENT ('.' IDENT)*
lit   := "'" … "'"
```

No `||`, no parens. Atoms today:
- `editor.focused`
- `find_bar.visible`
- `selection.is_caret`
- `shift.held`
- `language` (string: `"plain"` | `"markdown"` | language tag)

Add atoms in `command/src/context.rs::Context::flag` / `::lookup` and document them in `keymap` doc.

## `Context` trait surface

`Context` is intentionally narrow — handlers may not freely mutate `Window`. They may only:

```rs
trait Context {
    // Atoms / lookups
    fn flag(&self, key: &str) -> bool;
    fn lookup(&self, key: &str) -> Option<&str>;

    // Text mutation (every mutation flows through one of these)
    fn insert_text       (&mut self, text: &str)                       -> Result<(), Error>;
    fn delete_back       (&mut self)                                   -> Result<(), Error>;
    fn delete_forward    (&mut self)                                   -> Result<(), Error>;
    fn apply_selection_edit(&mut self, edit: SelectionEdit)            -> Result<(), Error>;

    // Selection / motion
    fn move_char/word/line/line_start/line_end/doc_start/doc_end (…)   -> Result<(), Error>;
    fn extend_*                                                          -> Result<(), Error>;
    fn add_cursor_above / below / at_next_match / at_all_matches         -> Result<(), Error>;
    fn column_select_up / down                                           -> Result<(), Error>;
    fn clear_secondary_cursors / select_word / line / paragraph / all    -> Result<(), Error>;

    // View
    fn view_toggle_*                                                     -> Result<(), Error>;
    fn view_adjust_zoom / reset_zoom / scroll_*                          -> Result<(), Error>;
    fn cycle_caret_style                                                 -> Result<(), Error>;

    // Overlays
    fn open_palette / quick_open / find / replace / goto_line / heading  -> Result<(), Error>;
    fn find_step / find_replace_one / find_replace_all / find_toggle / find_matches_to_cursors
                                                                    -> Result<(), Error>;

    // Files
    fn file_context(&mut self) -> Option<&mut dyn FileContext>;

    // Tabs / panes / windows / clipboard / spell …                       (see context.rs)
}

trait FileContext {
    fn file_open_dialog(&mut self)                          -> Result<(), Error>;
    fn file_open_paths(&mut self, paths: Vec<PathBuf>)       -> Result<(), Error>;
    fn file_open_folder(&mut self, path: Option<PathBuf>)    -> Result<(), Error>;
    fn toggle_file_tree(&mut self)                           -> Result<(), Error>;
    fn file_save / file_save_as / file_reload_external / ... -> Result<(), Error>;
}
```

Default impls return `Err(Error::UnsupportedContext("name"))` so headless tests can stub a partial `Context` without implementing every method.

## Edit pipeline contract

`SelectionEdit::*` → `crate::selection_edit::plan(buf, &edit) -> Option<SelectionEditPlan>`.

```rs
struct SelectionEditPlan {
    ops: Vec<EditOp>,                  // descending byte order
    selections_before: Vec<Selection>,
    selections_after:  Vec<Selection>,
}
```

Contract:
- `ops` is descending by `start` byte so sequential `Buffer::apply` keeps pre-edit offsets valid.
- `selections_before` is what the caller saw when planning began.
- `selections_after` is what `Buffer::set_selections` will be set to *after* all ops apply. Plan authors must shift positions through their own ops (legacy line-spanning indent/outdent used to ship `selections_before` unchanged — that bug was fixed in `edit_indent_shift`).
- Returning `Ok(None)` from a planner means "no effect" — no undo group is minted.

## Persistence client

```rs
struct PersistClient (clonable handle into the persist thread)

impl PersistClient {
    fn enqueue_edit_row(&self, …);
    fn enqueue_snapshot (&self, …);
    fn touch_buffer     (&self, id: BufferId, ts_ms: i64);
    fn delete_buffer    (&self, id: BufferId, ts_ms: i64);
    fn save_window / pane / tab / view_state (…);
    fn settings_get / put (…);
    fn keymap_get / put (…);
    fn theme_get / put (…);
    fn shutdown(self);     // drains, blocks until queue is empty
}
```

The persist thread enforces:
- ≥250 ms / ≥64 KiB batching window.
- Snapshot policy fires on 500 edits OR 256 KiB OR 60 s OR on close.
- Hot backup every 15 minutes via `rusqlite::backup`, 24 backups retained, then daily for 30 days.

## File-I/O contract

```rs
enum FileIoRequest {
    OpenFiles    { paths: Vec<PathBuf>, target_pane: Option<PaneId> },
    ListDirectory{ root: PathBuf, relative: PathBuf },
    SaveBuffer   { buffer_id: BufferId, path: PathBuf, content: String },
    ReloadBuffer { buffer_id: BufferId, path: PathBuf },
    WatchFile    { buffer_id: BufferId, file: FileAssociation },
    Shutdown,
}

enum FileIoEvent {
    Opened          { target_pane: Option<PaneId>, content: String, file: FileAssociation },
    DirectoryListed { root: PathBuf, relative: PathBuf,
                      entries: Vec<DirectoryEntry>, truncated: bool },
    Saved           { buffer_id: BufferId, file: FileAssociation },
    Reloaded        { buffer_id: BufferId, content: String, file: FileAssociation },
    ExternalChanged { buffer_id: BufferId, path: PathBuf, content: String, file: FileAssociation },
    Deleted         { buffer_id: BufferId, path: PathBuf },
    EncodingNotice  { path: PathBuf, encoding: &'static str },
    Failed          { operation: &'static str, path: Option<PathBuf>, reason: String },
}

struct DirectoryEntry {
    relative: PathBuf,
    name: String,
    kind: DirectoryEntryKind, // Directory | File
    size_bytes: Option<u64>,
}

struct StartupOpenedFile {
    content: String,
    file: FileAssociation,
    encoding_notice: Option<&'static str>,
}

struct FileAssociation {
    path: PathBuf,
    mtime_ms: i64,
    hash: u64,         // raw on-disk bytes; watcher/self-write detection
    content_hash: u64, // decoded rope text; dirty-tab decisions
}
```

UI threads poll their own `FileIoEvent` receiver on a `WM_TIMER` (250 ms cadence). File-I/O thread `notify`s file changes; UI presents a banner (`window_file::FileBanner`).

Startup file paths (`continuity.exe <path>`) do not enter the `FileIoRequest` queue. `app::main` reads file paths synchronously via `ui::file_io::read_startup_file`, asks core to create `OpenFileBuffer` buffers, and passes the resulting ids through the first `SpawnRequest.startup_open_buffer_ids`. Startup folder paths pass through `SpawnRequest.startup_folder_roots`; `ui::Window` opens the first folder in the file-tree pane after placement replay. Existing restored file-associated paths are canonicalized and deduped before import.

## Constraints + trade-offs
- **Reply-channel discipline** ⇒ every cross-thread call is observable in `cargo test` ⇒ verbose `Result<…, Error>` plumbing.
- **Grouped edit message** ⇒ replace-all and similar preplanned operations can land as one undo group and one core round-trip ⇒ callers must precompute valid descending ops or intentionally choose a whole-buffer replace.
- **Boxed `FnOnce` in `MutateSelections`** ⇒ flexible mutation closures ⇒ one heap allocation per call, but the call rate is bounded by user input (rare).
- **`Context` default-`UnsupportedContext`** ⇒ headless tests stub a few methods, full impl only in `Window` ⇒ adding a new method costs only the `Window` impl.

## Failure modes
- **Reply channel dropped** ⇒ `Error::ReplyDropped`; UI surfaces a banner; subsequent calls produce the same error until restart.
- **Handler panics** ⇒ caught at `Registry::dispatch`; logged; the editor keeps running.
- **Unknown command name** ⇒ `Error::UnknownCommand`; logged at dispatch; chord stays unbound (no-op).

## References
- `.docs/development/spec.md` §§2, 7 (threading + commands).
- `crates/command/src/context.rs` for the live method surface.
- `crates/core/src/message.rs` for the live `EditorMessage` enum.
- `.docs/design/features/command-system.md` for usage patterns + extension recipes.
