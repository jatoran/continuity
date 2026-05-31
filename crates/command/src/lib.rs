#![warn(missing_docs)]
//! Command registry, context predicates, and dispatch.
//!
//! Every editor action is a command. Keymaps, command palette, macros,
//! and tests all dispatch through this single entry point.

pub mod buffer_history;
pub mod clipboard;
pub mod context;
pub mod diagnostics;
pub mod editor;
pub mod editor_extras;
pub mod error;
pub mod file;
pub mod file_context;
pub mod find_context;
pub mod help;
pub mod id;
pub mod markdown;
pub mod markdown_inserters;
pub mod markdown_links_clipboard;
pub mod motion_extras;
pub mod panes;
pub mod predicate;
pub mod registry;
pub mod search;
pub mod selection;
pub mod settings;
pub mod spell;
pub mod tabs;
pub mod theme;
pub mod undo;
pub mod view;
pub mod view_context;
pub mod view_modes;
pub mod view_timeline_metrics;
pub mod windows;

pub use buffer_history::{register_buffer_history_commands, VIEW_BUFFER_HISTORY};
pub use clipboard::{
    register_clipboard_commands, EDITOR_COPY, EDITOR_CUT, EDITOR_PASTE, EDITOR_PASTE_AS_PLAIN_TEXT,
    EDITOR_PASTE_FROM_HISTORY,
};
pub use context::Context;
pub use diagnostics::{register_diagnostics_commands, DIAGNOSTICS_CAPTURE_LAYOUT};
pub use editor::{
    register_editor_primitives, register_keymap_commands, EDITOR_DELETE_BACK,
    EDITOR_DELETE_FORWARD, EDITOR_INSERT_CHAR, EDITOR_INSERT_NEWLINE, EDITOR_MOVE_CHAR_BACKWARD,
    EDITOR_MOVE_CHAR_FORWARD, EDITOR_MOVE_DOC_END, EDITOR_MOVE_DOC_START, EDITOR_MOVE_LINE_DOWN,
    EDITOR_MOVE_LINE_END, EDITOR_MOVE_LINE_START, EDITOR_MOVE_LINE_UP, KEYMAP_RELOAD,
    KEYMAP_SHOW_CONFLICTS,
};
pub use editor_extras::{
    register_rich_editing, EDITOR_CHANGE_CASE_LOWER, EDITOR_CHANGE_CASE_SENTENCE,
    EDITOR_CHANGE_CASE_TITLE, EDITOR_CHANGE_CASE_TOGGLE, EDITOR_CHANGE_CASE_UPPER,
    EDITOR_CONVERT_LINE_ENDINGS_CRLF, EDITOR_CONVERT_LINE_ENDINGS_LF, EDITOR_DELETE_TO_BRACKET,
    EDITOR_DELETE_TO_LINE_END, EDITOR_DELETE_TO_LINE_START, EDITOR_DELETE_WORD_BACKWARD,
    EDITOR_DELETE_WORD_FORWARD, EDITOR_DUPLICATE_LINE, EDITOR_DUPLICATE_SELECTION, EDITOR_INDENT,
    EDITOR_INSERT_NEWLINE_ABOVE, EDITOR_INSERT_NEWLINE_BELOW, EDITOR_INSERT_NEWLINE_SMART,
    EDITOR_JOIN_LINES, EDITOR_MOVE_LINE_DOWN_BLOCK, EDITOR_MOVE_LINE_UP_BLOCK, EDITOR_OUTDENT,
    EDITOR_REFLOW_PARAGRAPH, EDITOR_REVERSE_LINES, EDITOR_SHUFFLE_LINES, EDITOR_SORT_LINES_ASC,
    EDITOR_SORT_LINES_ASC_CI, EDITOR_SORT_LINES_ASC_LEN, EDITOR_SORT_LINES_ASC_NUM,
    EDITOR_SORT_LINES_DESC, EDITOR_SORT_LINES_DESC_CI, EDITOR_SORT_LINES_DESC_LEN,
    EDITOR_SORT_LINES_DESC_NUM, EDITOR_SPACES_TO_TABS, EDITOR_SURROUND_BRACES,
    EDITOR_SURROUND_BRACKETS, EDITOR_SURROUND_DOUBLE_QUOTES, EDITOR_SURROUND_PARENS,
    EDITOR_SURROUND_SELECTION_WITH, EDITOR_TABS_TO_SPACES, EDITOR_TRANSPOSE_CHARS,
    EDITOR_TRANSPOSE_WORDS, EDITOR_TRIM_TRAILING_WHITESPACE, EDITOR_UNIQUE_LINES,
    EDITOR_WRAP_AT_COLUMN,
};
pub use error::Error;
pub use file::{
    register_file_commands, FILE_KEEP_MINE, FILE_OPEN, FILE_OPEN_FOLDER, FILE_RELOAD_EXTERNAL,
    FILE_SAVE, FILE_SAVE_AS, FILE_SHOW_DIFF,
};
pub use file_context::FileContext;
pub use find_context::FindContext;
pub use help::{register_help_commands, HELP_TUTORIAL, TUTORIAL_MD};
pub use id::CommandId;
pub use markdown::{
    register_markdown_commands, MARKDOWN_CYCLE_HEADING_DOWN, MARKDOWN_CYCLE_HEADING_UP,
    MARKDOWN_CYCLE_LIST_MARKER, MARKDOWN_DEMOTE_SECTION, MARKDOWN_INSERT_CODE_FENCE,
    MARKDOWN_INSERT_IMAGE_REF, MARKDOWN_INSERT_LINK, MARKDOWN_MOVE_SECTION_DOWN,
    MARKDOWN_MOVE_SECTION_UP, MARKDOWN_PROMOTE_SECTION, MARKDOWN_RENUMBER_LIST,
    MARKDOWN_SET_HEADING_1, MARKDOWN_SET_HEADING_2, MARKDOWN_SET_HEADING_3, MARKDOWN_SET_HEADING_4,
    MARKDOWN_SET_HEADING_5, MARKDOWN_SET_HEADING_6, MARKDOWN_TABLE_CARET_CELL_END,
    MARKDOWN_TABLE_CARET_CELL_START, MARKDOWN_TABLE_DELETE_COL, MARKDOWN_TABLE_DELETE_ROW,
    MARKDOWN_TABLE_DELETE_TABLE, MARKDOWN_TABLE_EXTEND_CELL_END, MARKDOWN_TABLE_EXTEND_CELL_START,
    MARKDOWN_TABLE_INSERT_COL_LEFT, MARKDOWN_TABLE_INSERT_COL_RIGHT,
    MARKDOWN_TABLE_INSERT_ROW_ABOVE, MARKDOWN_TABLE_INSERT_ROW_BELOW, MARKDOWN_TABLE_SELECT_CELL,
    MARKDOWN_TOGGLE_BOLD, MARKDOWN_TOGGLE_BULLET, MARKDOWN_TOGGLE_CHECKBOX,
    MARKDOWN_TOGGLE_INLINE_CODE, MARKDOWN_TOGGLE_ITALIC, MARKDOWN_TOGGLE_NUMBERED,
    MARKDOWN_TOGGLE_STRIKETHROUGH, MARKDOWN_WRAP_IN_BLOCKQUOTE,
};
pub use markdown_links_clipboard::{
    register_markdown_links_clipboard, EDITOR_COPY_RENDERED_TEXT, EDITOR_COPY_SOURCE_TEXT,
    EDITOR_OPEN_LINK_AT_CARET, MARKDOWN_COPY_AS_HTML,
};
pub use motion_extras::{
    register_motion_extras, EDITOR_EXTEND_PARAGRAPH_BACKWARD, EDITOR_EXTEND_PARAGRAPH_FORWARD,
    EDITOR_EXTEND_SMART_HOME, EDITOR_EXTEND_WORD_BACKWARD, EDITOR_EXTEND_WORD_FORWARD,
    EDITOR_MOVE_PARAGRAPH_BACKWARD, EDITOR_MOVE_PARAGRAPH_FORWARD, EDITOR_MOVE_WORD_BACKWARD,
    EDITOR_MOVE_WORD_FORWARD, EDITOR_SHRINK_SELECTION_SMART, EDITOR_SMART_HOME,
};
pub use panes::{
    register_pane_commands, LAYOUT_FOUR_COLS, LAYOUT_GRID_2X2, LAYOUT_GRID_2X4, LAYOUT_SINGLE,
    LAYOUT_THREE_COLS, LAYOUT_TWO_COLS, LAYOUT_TWO_ROWS, PANE_CLOSE, PANE_FOCUS_DOWN,
    PANE_FOCUS_LEFT, PANE_FOCUS_RIGHT, PANE_FOCUS_UP, PANE_MAXIMIZE_TOGGLE, PANE_RESIZE_DOWN,
    PANE_RESIZE_LEFT, PANE_RESIZE_RIGHT, PANE_RESIZE_UP, PANE_SPLIT_HORIZONTAL,
    PANE_SPLIT_VERTICAL,
};
pub use predicate::ContextPredicate;
pub use registry::{Args, Handler, Registry};
pub use search::{
    register_search_commands, EDITOR_FIND, EDITOR_FIND_IN_ALL, EDITOR_FIND_NEXT, EDITOR_FIND_PREV,
    EDITOR_FIND_REPLACE_ALL, EDITOR_FIND_REPLACE_ONE, EDITOR_GOTO_HEADING, EDITOR_GOTO_LINE,
    EDITOR_REPLACE, OVERLAY_DISMISS, PALETTE_SHOW, QUICK_OPEN_SHOW,
};
pub use selection::{
    register_selection_commands, EDITOR_ADD_CURSOR_ABOVE, EDITOR_ADD_CURSOR_AT_ALL_MATCHES,
    EDITOR_ADD_CURSOR_AT_NEXT_MATCH, EDITOR_ADD_CURSOR_BELOW, EDITOR_CLEAR_SECONDARY_CURSORS,
    EDITOR_COLUMN_SELECT_DOWN, EDITOR_COLUMN_SELECT_UP, EDITOR_EXPAND_SELECTION_SMART,
    EDITOR_EXTEND_CHAR_BACKWARD, EDITOR_EXTEND_CHAR_FORWARD, EDITOR_EXTEND_DOC_END,
    EDITOR_EXTEND_DOC_START, EDITOR_EXTEND_LINE_DOWN, EDITOR_EXTEND_LINE_END,
    EDITOR_EXTEND_LINE_START, EDITOR_EXTEND_LINE_UP, EDITOR_SELECT_ALL, EDITOR_SELECT_LINE,
    EDITOR_SELECT_PARAGRAPH, EDITOR_SELECT_WORD,
};
pub use settings::{register_settings_commands, KEYMAP_RELOAD_LAYERED, SETTINGS_OPEN};
pub use spell::{
    register_spell_commands, SPELL_ADD_TO_DICTIONARY, SPELL_REPLACE_AT_CARET,
    SPELL_SHOW_SUGGESTIONS, SPELL_TOGGLE,
};
pub use tabs::{
    register_tab_commands, TAB_CLOSE, TAB_GO_TO_1, TAB_GO_TO_2, TAB_GO_TO_3, TAB_GO_TO_4,
    TAB_GO_TO_5, TAB_GO_TO_6, TAB_GO_TO_7, TAB_GO_TO_8, TAB_GO_TO_9, TAB_MRU_NEXT, TAB_MRU_PREV,
    TAB_NEW, TAB_NEXT, TAB_PREV, TAB_REOPEN_CLOSED,
};
pub use theme::{
    register_theme_commands, THEME_CLONE, THEME_CREATE_BLANK, THEME_DELETE, THEME_DUPLICATE,
    THEME_EDIT, THEME_RENAME, THEME_REVEAL_FOLDER,
};
pub use undo::{
    register_undo_commands, EDITOR_REDO, EDITOR_REDO_ALTERNATE_BRANCH, EDITOR_UNDO,
    EDITOR_UNDO_TREE_PICK,
};
pub use view::{
    register_view_commands, VIEW_CYCLE_THEME, VIEW_SCROLL_DOC_END, VIEW_SCROLL_DOC_START,
    VIEW_SCROLL_LINE_DOWN, VIEW_SCROLL_LINE_UP, VIEW_SCROLL_PAGE_DOWN, VIEW_SCROLL_PAGE_UP,
    VIEW_SET_FONT_FAMILY, VIEW_SET_FONT_SIZE, VIEW_TOGGLE_ALL_LINE_NUMBERS, VIEW_TOGGLE_FILE_TREE,
    VIEW_TOGGLE_INDENT_GUIDES, VIEW_TOGGLE_LINE_NUMBERS, VIEW_TOGGLE_MINIMAP,
    VIEW_TOGGLE_RELATIVE_LINE_NUMBERS, VIEW_TOGGLE_WHITESPACE, VIEW_TOGGLE_WRAP, VIEW_ZOOM_IN,
    VIEW_ZOOM_OUT, VIEW_ZOOM_RESET,
};
pub use view_context::ViewContext;
pub use view_modes::{
    register_view_modes_commands, VIEW_CYCLE_FOCUS, VIEW_FOCUS_LINE, VIEW_FOCUS_OFF,
    VIEW_FOCUS_PARAGRAPH, VIEW_FOCUS_SENTENCE, VIEW_FOLD, VIEW_FOLD_ALL, VIEW_SLASH_PALETTE_SHOW,
    VIEW_TAB_OVERLAY_SHOW, VIEW_TOGGLE_DISTRACTION_FREE, VIEW_TOGGLE_FOCUS_MODE,
    VIEW_TOGGLE_FOLD_AT_CARET, VIEW_UNFOLD, VIEW_UNFOLD_ALL,
};
pub use view_timeline_metrics::{
    register_view_timeline_metrics_commands, BUFFER_MARK_SNAPSHOT, BUFFER_TIMELINE, METRICS_PURGE,
    VIEW_METRICS,
};
pub use windows::{
    register_window_commands, NewWindowHandler, TearOffHandler, WINDOW_NEW_WINDOW,
    WINDOW_TEAR_OFF_FOCUSED_TAB,
};

/// Build a [`Registry`] with every command this crate registers — every
/// `register_*_commands` function called in order. Used by
/// `xtask gen-tutorial` to enumerate command ids, descriptions, and
/// palette-safe flags without depending on the UI crate.
///
/// `register_window_commands` requires app-supplied closures (new-
/// window spawn, tear-off); for documentation purposes both are
/// stubbed with `UnsupportedContext` errors. The stubs are never
/// invoked from xtask — the registry is read-only.
#[must_use]
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    register_editor_primitives(&mut registry);
    register_diagnostics_commands(&mut registry);
    register_selection_commands(&mut registry);
    register_keymap_commands(&mut registry);
    register_motion_extras(&mut registry);
    register_rich_editing(&mut registry);
    register_markdown_commands(&mut registry);
    register_markdown_links_clipboard(&mut registry);
    register_undo_commands(&mut registry);
    register_view_commands(&mut registry);
    register_search_commands(&mut registry);
    register_settings_commands(&mut registry);
    register_pane_commands(&mut registry);
    register_tab_commands(&mut registry);
    register_theme_commands(&mut registry);
    register_file_commands(&mut registry);
    register_clipboard_commands(&mut registry);
    register_spell_commands(&mut registry);
    register_help_commands(&mut registry);
    register_buffer_history_commands(&mut registry);
    let stub_new = |_: &serde_json::Value, _: &mut dyn Context| {
        Err(Error::UnsupportedContext("window.new_window (docs stub)"))
    };
    let stub_tear = |_: &serde_json::Value, _: &mut dyn Context| {
        Err(Error::UnsupportedContext(
            "window.tear_off_focused_tab (docs stub)",
        ))
    };
    register_window_commands(&mut registry, stub_new, stub_tear);
    registry
}
