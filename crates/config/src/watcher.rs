//! `SettingsWatcher` — `notify`-backed live-reload for `settings.toml`,
//! `keymap.toml`, and the `themes/` directory.
//!
//! ## Thread ownership
//!
//! - The `notify` recommended watcher spawns its own background thread
//!   (the `notify` crate owns it). Its event closure is the *single
//!   producer* on a `crossbeam_channel::Sender<RawNotifyEvent>`.
//! - `SettingsWatcher::spawn` also owns one *debounce thread*: a
//!   `std::thread::Builder` named `continuity-config-watcher` that drains
//!   the raw events, debounces them on a fixed window, parses + validates
//!   the affected file, and emits a [`ConfigEvent`] on the *output*
//!   channel.
//! - The output `Receiver<ConfigEvent>` is the consumer surface — typically
//!   the UI thread polls it from a `WM_TIMER` tick.
//!
//! ## Why two threads
//!
//! `notify`'s `EventHandler` runs synchronously inside the watcher
//! thread; doing TOML I/O and validation there would stall further
//! events. The debounce thread keeps that work off the `notify` thread
//! while still using a single-writer model end-to-end.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, select, tick, Receiver, Sender};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::Error;

/// Default debounce: editors save in 1–2 syscalls, but cross-platform
/// editors (vim+swap, jetbrains, vscode) frequently emit a `Remove +
/// Create` pair. 150 ms collapses the burst without making save→reload
/// feel laggy.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(150);

/// One change observed by [`SettingsWatcher`]. The payload is always
/// pre-parsed: the watcher emits `Err` on parse / validation failures
/// rather than raw bytes the consumer would have to handle.
///
/// Cloneable so the registry can fan an event out to every live window
/// (Phase 16.5).
#[derive(Clone, Debug)]
pub enum ConfigEvent {
    /// `settings.toml` changed and re-parsed cleanly. The new
    /// [`crate::Settings`] is already validated. Boxed because the
    /// `Settings` struct is significantly larger than the other
    /// variants — keep the enum cheap to pass through channels.
    Settings(Box<crate::Settings>),
    /// `keymap.toml` changed. Body is the file's text — the keymap crate
    /// owns the parser.
    Keymap(String),
    /// A theme TOML inside the watched themes directory changed.
    Theme {
        /// The theme's stem (file name without `.toml`).
        name: String,
        /// File contents. The theme crate owns the parser.
        toml: String,
    },
    /// A reload attempt failed. Carries the file kind and the diagnostic
    /// so consumers can show a banner instead of silently desyncing.
    Failed {
        /// The TOML file the failure was attached to (best-effort path).
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },
}

/// Paths the watcher should monitor.
#[derive(Debug, Clone)]
pub struct WatchPaths {
    /// Path to `settings.toml`. Watched as a file.
    pub settings: PathBuf,
    /// Path to `keymap.toml`. Watched as a file.
    pub keymap: PathBuf,
    /// Path to a `themes/` directory. Watched non-recursively.
    pub themes_dir: PathBuf,
}

/// Live-reload watcher. Owns the `notify` watcher and the debounce
/// thread; drop to shut both down cleanly.
///
/// Thread ownership: the join handle for the debounce thread lives here;
/// the `notify::RecommendedWatcher` owns its own background thread.
pub struct SettingsWatcher {
    _watcher: RecommendedWatcher,
    join: Option<JoinHandle<()>>,
    shutdown: Sender<()>,
    /// The output channel. Cloned on construction so callers can keep
    /// the `Receiver` in their own message loop.
    rx: Receiver<ConfigEvent>,
}

impl SettingsWatcher {
    /// Spawn a watcher over `paths`. Channel capacity controls how many
    /// outbound events can queue before the debounce thread blocks.
    /// 32 is plenty in practice (one save burst is one event).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Watch`] if `notify` cannot install the watch on
    /// any of the requested paths. The settings TOML's parent directory
    /// is watched (rather than the file itself) so atomic-replace saves
    /// are not lost.
    pub fn spawn(paths: WatchPaths) -> Result<Self, Error> {
        Self::spawn_with_debounce(paths, DEFAULT_DEBOUNCE)
    }

    /// Same as [`Self::spawn`] with a custom debounce window. Tests use
    /// a small window (10 ms) to keep them fast.
    ///
    /// # Errors
    ///
    /// See [`Self::spawn`].
    pub fn spawn_with_debounce(paths: WatchPaths, debounce: Duration) -> Result<Self, Error> {
        let (raw_tx, raw_rx) = bounded::<RawEvent>(256);
        let raw_tx_for_handler = raw_tx.clone();

        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(ev) => {
                        if matches!(
                            ev.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        ) {
                            for path in ev.paths {
                                // Best-effort: drop on full queue. The debounce
                                // thread coalesces by path so we can lose
                                // duplicates without losing the event itself.
                                let _ = raw_tx_for_handler.try_send(RawEvent { path });
                            }
                        }
                    }
                    Err(e) => eprintln!("continuity-config: notify error: {e}"),
                }
            })?;

        // Watch each parent directory non-recursively. File-level watches
        // miss atomic-replace saves on Windows (notepad, vscode).
        let watch_dirs = collect_dirs(&paths);
        for dir in &watch_dirs {
            if dir.exists() {
                watcher.watch(dir, RecursiveMode::NonRecursive)?;
            }
        }

        let (out_tx, out_rx) = bounded::<ConfigEvent>(32);
        let (sd_tx, sd_rx) = bounded::<()>(1);
        let paths_arc = Arc::new(paths);
        let join = thread::Builder::new()
            .name("continuity-config-watcher".into())
            .spawn(move || debounce_loop(raw_rx, sd_rx, out_tx, paths_arc, debounce))
            .expect("spawn continuity-config-watcher thread");

        Ok(Self {
            _watcher: watcher,
            join: Some(join),
            shutdown: sd_tx,
            rx: out_rx,
        })
    }

    /// The consumer-side receiver. Never `Send`-shared mutably; clone
    /// at most once per consumer thread.
    #[must_use]
    pub fn events(&self) -> Receiver<ConfigEvent> {
        self.rx.clone()
    }
}

impl Drop for SettingsWatcher {
    fn drop(&mut self) {
        // Best-effort shutdown signal; the debounce loop exits when the
        // raw_rx side closes too (notify::Watcher drop closes it).
        let _ = self.shutdown.try_send(());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// One raw notify event reduced to the path that triggered it.
struct RawEvent {
    path: PathBuf,
}

/// What kind of file a path corresponds to.
enum Match {
    Settings,
    Keymap,
    Theme(String),
    None,
}

fn classify(path: &Path, paths: &WatchPaths) -> Match {
    if same_file(path, &paths.settings) {
        Match::Settings
    } else if same_file(path, &paths.keymap) {
        Match::Keymap
    } else if path.parent() == Some(paths.themes_dir.as_path())
        && path.extension().is_some_and(|ext| ext == "toml")
    {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Match::Theme(stem)
    } else {
        Match::None
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    // Some platforms canonicalize differently — fall back to the
    // file-name match when canonicalize fails (e.g. file was atomically
    // replaced and is briefly missing).
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.file_name() == b.file_name() && a.parent() == b.parent(),
    }
}

fn collect_dirs(paths: &WatchPaths) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(3);
    for parent in [
        paths.settings.parent(),
        paths.keymap.parent(),
        Some(paths.themes_dir.as_path()),
    ]
    .into_iter()
    .flatten()
    {
        let p = parent.to_path_buf();
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn debounce_loop(
    raw_rx: Receiver<RawEvent>,
    shutdown_rx: Receiver<()>,
    out_tx: Sender<ConfigEvent>,
    paths: Arc<WatchPaths>,
    debounce: Duration,
) {
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    // Tick a bit faster than the debounce so we don't add observable
    // latency on top of it.
    let tick_interval = (debounce / 2).max(Duration::from_millis(5));
    let ticker = tick(tick_interval);

    loop {
        select! {
            recv(shutdown_rx) -> _ => break,
            recv(raw_rx) -> msg => {
                match msg {
                    Ok(ev) => {
                        pending.insert(ev.path, Instant::now());
                    }
                    Err(_) => break, // notify watcher dropped
                }
            }
            recv(ticker) -> _ => {
                let now = Instant::now();
                let due: Vec<PathBuf> = pending
                    .iter()
                    .filter(|(_, t)| now.duration_since(**t) >= debounce)
                    .map(|(p, _)| p.clone())
                    .collect();
                for path in due {
                    pending.remove(&path);
                    let Some(event) = process_path(&path, &paths) else {
                        // Path isn't one of the watched config files
                        // (e.g. a SQLite WAL touch in a shared parent
                        // directory). Drop silently — surfacing it as a
                        // banner is noise, not signal.
                        continue;
                    };
                    if out_tx.send(event).is_err() {
                        return; // consumer dropped
                    }
                }
            }
        }
    }
}

fn process_path(path: &Path, paths: &WatchPaths) -> Option<ConfigEvent> {
    match classify(path, paths) {
        Match::Settings => Some(match std::fs::read_to_string(&paths.settings) {
            Ok(text) => match crate::Settings::from_toml_validated(&text) {
                Ok(s) => ConfigEvent::Settings(Box::new(s)),
                Err(e) => ConfigEvent::Failed {
                    path: paths.settings.clone(),
                    reason: e.to_string(),
                },
            },
            Err(e) => ConfigEvent::Failed {
                path: paths.settings.clone(),
                reason: e.to_string(),
            },
        }),
        Match::Keymap => Some(match std::fs::read_to_string(&paths.keymap) {
            Ok(text) => ConfigEvent::Keymap(text),
            Err(e) => ConfigEvent::Failed {
                path: paths.keymap.clone(),
                reason: e.to_string(),
            },
        }),
        Match::Theme(name) => {
            let theme_path = paths.themes_dir.join(format!("{name}.toml"));
            Some(match std::fs::read_to_string(&theme_path) {
                Ok(text) => ConfigEvent::Theme { name, toml: text },
                Err(e) => ConfigEvent::Failed {
                    path: theme_path,
                    reason: e.to_string(),
                },
            })
        }
        // Filesystem events on unrelated files in a watched parent
        // directory (e.g. `continuity.db-wal` co-located with
        // `settings.toml`) classify here. Drop them — they aren't
        // watched config files, so they should produce no event.
        Match::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognises_each_kind() {
        let paths = WatchPaths {
            settings: PathBuf::from("/cfg/settings.toml"),
            keymap: PathBuf::from("/cfg/keymap.toml"),
            themes_dir: PathBuf::from("/cfg/themes"),
        };
        assert!(matches!(
            classify(Path::new("/cfg/settings.toml"), &paths),
            Match::Settings
        ));
        assert!(matches!(
            classify(Path::new("/cfg/keymap.toml"), &paths),
            Match::Keymap
        ));
        assert!(matches!(
            classify(Path::new("/cfg/themes/paper.toml"), &paths),
            Match::Theme(ref n) if n == "paper"
        ));
        assert!(matches!(
            classify(Path::new("/cfg/other.toml"), &paths),
            Match::None
        ));
    }

    #[test]
    fn collect_dirs_deduplicates() {
        let paths = WatchPaths {
            settings: PathBuf::from("/cfg/settings.toml"),
            keymap: PathBuf::from("/cfg/keymap.toml"),
            themes_dir: PathBuf::from("/cfg/themes"),
        };
        let dirs = collect_dirs(&paths);
        assert_eq!(dirs.len(), 2); // /cfg appears once, /cfg/themes once
        assert!(dirs.iter().any(|d| d.ends_with("themes")));
    }
}
