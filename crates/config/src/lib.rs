#![warn(missing_docs)]
//! `settings.toml` loader, validator, typed views, and live-reload watcher.

pub mod autocorrect;
pub mod effective_autocorrect;
pub mod error;
pub mod focus;
pub mod markdown_paths;
pub mod mode;
pub mod settings;
pub mod smart_typography;
pub mod validate;
pub mod watcher;
pub mod workers;

pub use autocorrect::{
    first_match as autocorrect_first_match, is_autocorrect_trigger, AutocorrectMatch,
    AutocorrectRule, AutocorrectRuleset,
};
pub use error::Error;
pub use focus::FocusConfig;
pub use mode::{
    CaretStyle, FocusMode, MarkdownDialect, PersistenceMode, RevealMode, StatusBarSegment,
    TabCloseButton, ThemeMode,
};
pub use settings::{
    BackupConfig, EditorConfig, FindConfig, MarkdownConfig, PersistenceConfig, Settings,
    StatusBarConfig, UiConfig, WindowConfig,
};
pub use smart_typography::smart_typography_rules;
pub use watcher::{ConfigEvent, SettingsWatcher, WatchPaths, DEFAULT_DEBOUNCE};
pub use workers::WorkerConfig;
