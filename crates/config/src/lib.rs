#![warn(missing_docs)]
//! `settings.toml` loader, validator, typed views, and live-reload watcher.

pub mod autocorrect;
pub mod effective_autocorrect;
pub mod error;
pub mod focus;
pub mod markdown_paths;
pub mod mode;
pub mod settings;
pub mod settings_backup;
pub mod settings_markdown;
pub mod settings_window;
pub mod smart_typography;
pub mod validate;
pub mod watcher;
pub mod workers;
pub mod zoom;

pub use autocorrect::{
    first_match as autocorrect_first_match, is_autocorrect_trigger, AutocorrectMatch,
    AutocorrectRule, AutocorrectRuleset,
};
pub use error::Error;
pub use focus::FocusConfig;
pub use mode::{
    CaretStyle, FocusMode, IndentType, MarkdownDialect, PersistenceMode, RevealMode,
    StatusBarSegment, TabCloseButton, ThemeMode,
};
pub use settings::{
    EditorConfig, FindConfig, PersistenceConfig, Settings, StatusBarConfig, UiConfig,
};
pub use settings_backup::BackupConfig;
pub use settings_markdown::MarkdownConfig;
pub use settings_window::WindowConfig;
pub use smart_typography::smart_typography_rules;
pub use watcher::{ConfigEvent, SettingsWatcher, WatchPaths, DEFAULT_DEBOUNCE};
pub use workers::WorkerConfig;
pub use zoom::{MAX_ZOOM, MIN_ZOOM};
