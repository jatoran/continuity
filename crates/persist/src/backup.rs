//! Hot-backup scheduler.
//!
//! Spawns a thread that wakes on a fixed cadence and asks the persistence
//! thread to mirror the live database to the backups directory using
//! [`Store::online_backup`](crate::store::Store::online_backup). Retains the
//! most recent N backups; older files are deleted.
//!
//! Per the spec (§4 hot backup) this would also keep daily backups for 30
//! days; the daily tier is tracked as a Phase 17/18 follow-up.

use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::handle::PersistClient;
use crate::Error;

/// Default cadence between backup attempts. Per spec §4 ("every 15 minutes
/// if the DB has changed").
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Default number of rolling backups to keep on disk.
pub const DEFAULT_RETAIN: usize = 24;

/// Configuration for the backup scheduler.
#[derive(Debug, Clone)]
pub struct BackupConfig {
    /// Directory backups are written to. Created if missing.
    pub directory: PathBuf,
    /// How often to attempt a backup.
    pub interval: Duration,
    /// How many backups to retain.
    pub retain: usize,
}

impl BackupConfig {
    /// Construct with the spec defaults (15 minutes / 24 retained).
    #[must_use]
    pub fn with_defaults(directory: PathBuf) -> Self {
        Self {
            directory,
            interval: DEFAULT_INTERVAL,
            retain: DEFAULT_RETAIN,
        }
    }
}

/// Handle to a running backup scheduler. Drop to stop the thread.
pub struct BackupScheduler {
    stop_tx: Sender<()>,
    stop_flag: Arc<AtomicBool>,
    /// Phase-16.5 owner-message channel: a typed update (interval +
    /// retain count) sent to the loop. Sent best-effort; the worst case
    /// is the next backup uses the previous cadence.
    config_tx: Sender<BackupConfig>,
    join: Option<JoinHandle<()>>,
}

impl BackupScheduler {
    /// Spawn a scheduler thread that periodically asks `persist` to back up
    /// the database according to `config`.
    ///
    /// Returns immediately; backups happen on the spawned thread. The first
    /// backup fires after `config.interval` (not immediately at spawn).
    pub fn spawn(persist: PersistClient, config: BackupConfig) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let (config_tx, config_rx) = bounded::<BackupConfig>(4);
        let stop_flag_thread = Arc::clone(&stop_flag);
        let join = thread::Builder::new()
            .name("continuity-backup".into())
            .spawn(move || backup_loop(persist, config, &stop_rx, &config_rx, &stop_flag_thread))
            .expect("spawn continuity-backup thread");
        Self {
            stop_tx,
            stop_flag,
            config_tx,
            join: Some(join),
        }
    }

    /// Phase-16.5 typed owner message: update the running scheduler's
    /// cadence + retention without restarting it. The new config takes
    /// effect after the in-flight wait elapses.
    pub fn set_config(&self, config: BackupConfig) {
        // Best-effort: scheduler may have already shut down.
        let _ = self.config_tx.try_send(config);
    }

    /// Stop the scheduler and wait for the thread to exit. Idempotent.
    pub fn shutdown(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        let _ = self.stop_tx.send(());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for BackupScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn backup_loop(
    persist: PersistClient,
    initial_config: BackupConfig,
    stop_rx: &Receiver<()>,
    config_rx: &Receiver<BackupConfig>,
    stop_flag: &Arc<AtomicBool>,
) {
    let mut config = initial_config;
    if let Err(e) = std::fs::create_dir_all(&config.directory) {
        eprintln!(
            "continuity-backup: cannot create backups dir {}: {e}",
            config.directory.display()
        );
        return;
    }
    loop {
        // Wait until the configured interval elapses, a config update is
        // available, or shutdown is signaled. select! lets a config
        // change shorten / lengthen the next wait without missing a
        // shutdown.
        crossbeam_channel::select! {
            recv(stop_rx) -> _ => return,
            recv(config_rx) -> msg => {
                if let Ok(new_config) = msg {
                    if new_config.directory != config.directory {
                        if let Err(e) = std::fs::create_dir_all(&new_config.directory) {
                            eprintln!(
                                "continuity-backup: cannot create backups dir {}: {e}",
                                new_config.directory.display()
                            );
                        }
                    }
                    config = new_config;
                }
                continue;
            },
            default(config.interval) => {}
        }
        if stop_flag.load(Ordering::Acquire) {
            return;
        }
        if let Err(e) = run_one_backup(&persist, &config) {
            eprintln!("continuity-backup: backup failed: {e}");
        }
    }
}

fn run_one_backup(persist: &PersistClient, config: &BackupConfig) -> Result<(), Error> {
    let id = next_session_id(&config.directory);
    let dest = config.directory.join(format!("session-{id:06}.db"));
    persist.backup(dest)?;
    if let Err(e) = persist_session_id(&config.directory, id) {
        eprintln!("continuity-backup: cannot persist session id: {e}");
    }
    enforce_retention(&config.directory, config.retain);
    Ok(())
}

/// Pick the next monotonic session id by reading a sidecar file. If the file
/// doesn't exist or is unreadable, scans existing `session-N.db` filenames
/// and returns `max + 1`.
fn next_session_id(dir: &Path) -> u64 {
    if let Ok(text) = std::fs::read_to_string(dir.join("next-id")) {
        if let Ok(n) = text.trim().parse::<u64>() {
            return n;
        }
    }
    scan_max_session_id(dir).map(|n| n + 1).unwrap_or(1)
}

fn scan_max_session_id(dir: &Path) -> Option<u64> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut max = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(rest) = name
            .strip_prefix("session-")
            .and_then(|s| s.strip_suffix(".db"))
        {
            if let Ok(n) = rest.parse::<u64>() {
                max = Some(max.map_or(n, |m: u64| m.max(n)));
            }
        }
    }
    max
}

fn persist_session_id(dir: &Path, used_id: u64) -> std::io::Result<()> {
    std::fs::write(dir.join("next-id"), format!("{}\n", used_id + 1))
}

/// Delete all but the newest `retain` `session-N.db` files in `dir`.
fn enforce_retention(dir: &Path, retain: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name_str = name.to_str()?;
            let rest = name_str
                .strip_prefix("session-")
                .and_then(|s| s.strip_suffix(".db"))?;
            let id = rest.parse::<u64>().ok()?;
            Some((id, e.path()))
        })
        .collect();
    files.sort_by_key(|(id, _)| *id);
    if files.len() <= retain {
        return;
    }
    let drop_count = files.len() - retain;
    for (_, path) in files.into_iter().take(drop_count) {
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!(
                "continuity-backup: cannot remove old backup {}: {e}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_session_id_starts_at_one() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(next_session_id(dir.path()), 1);
    }

    #[test]
    fn next_session_id_reads_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("next-id"), "42\n").unwrap();
        assert_eq!(next_session_id(dir.path()), 42);
    }

    #[test]
    fn next_session_id_falls_back_to_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("session-000007.db"), b"x").unwrap();
        std::fs::write(dir.path().join("session-000003.db"), b"x").unwrap();
        assert_eq!(next_session_id(dir.path()), 8);
    }

    #[test]
    fn enforce_retention_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=5u64 {
            std::fs::write(dir.path().join(format!("session-{i:06}.db")), b"x").unwrap();
        }
        enforce_retention(dir.path(), 2);
        let mut remaining: Vec<u64> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name();
                let s = n.to_str()?;
                let r = s.strip_prefix("session-")?.strip_suffix(".db")?;
                r.parse().ok()
            })
            .collect();
        remaining.sort_unstable();
        assert_eq!(remaining, vec![4, 5]);
    }

    #[test]
    fn end_to_end_writes_and_retains() {
        use crate::handle::PersistHandle;
        let live_dir = tempfile::tempdir().unwrap();
        let backup_dir = tempfile::tempdir().unwrap();
        let persist = PersistHandle::spawn(&live_dir.path().join("live.db")).unwrap();
        let client = persist.client();

        let config = BackupConfig {
            directory: backup_dir.path().to_path_buf(),
            interval: Duration::from_millis(50),
            retain: 2,
        };
        let mut sched = BackupScheduler::spawn(client, config);

        // Wait for at least 3 backups to have occurred.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let count = std::fs::read_dir(backup_dir.path())
                .unwrap()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|s| s.starts_with("session-") && s.ends_with(".db"))
                })
                .count();
            if count >= 2 {
                break;
            }
            if Instant::now() > deadline {
                panic!("backup scheduler did not produce backups within deadline");
            }
            thread::sleep(Duration::from_millis(50));
        }
        sched.shutdown();

        // Retention: at most 2 session-*.db files should remain.
        let count = std::fs::read_dir(backup_dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|s| s.starts_with("session-") && s.ends_with(".db"))
            })
            .count();
        assert!(count <= 2, "retention not enforced: {count} files");
    }
}
