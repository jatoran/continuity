//! SQL-level online-backup driver for [`super::Store`].
//!
//! Mirrors the live database to an arbitrary destination path through
//! `rusqlite::backup::Backup`. Safe under concurrent writes — pages
//! are copied through the same connection, no file-copy races. The
//! top-level [`crate::backup`] scheduler is what orchestrates cadence
//! and retention; this module only knows how to take one snapshot.
//!
//! Thread ownership: the persistence thread (same as the parent
//! `Store`).

use std::path::Path;
use std::time::Duration;

use rusqlite::{backup::Backup, Connection};

use super::Store;
use crate::Error;

impl Store {
    /// Mirror the live database to `dest_path` using SQLite's online backup
    /// API. Safe under concurrent writes — pages are copied through the same
    /// connection, no file-copy races.
    pub(crate) fn online_backup(&self, dest_path: &Path) -> Result<(), Error> {
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut dest = Connection::open(dest_path)?;
        let backup = Backup::new(&self.conn, &mut dest)?;
        backup.run_to_completion(64, Duration::from_millis(0), None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use continuity_buffer::Buffer;
    use continuity_text::{EditOp, Position};

    use super::*;

    #[test]
    fn online_backup_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live.db");
        let backup_path = dir.path().join("backup.db");
        let store = Store::open(&live).unwrap();
        let mut buf = Buffer::from_text("hello");
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();
        buf.apply(&EditOp::insert(Position::new(0, 5), "!"))
            .unwrap();
        store.save_snapshot(buf.id(), &buf.snapshot()).unwrap();

        store.online_backup(&backup_path).unwrap();
        let restored = Store::open(&backup_path).unwrap();
        let snap = restored
            .load_latest_valid_snapshot(buf.id())
            .unwrap()
            .unwrap();
        assert_eq!(snap.content, "hello!");
    }
}
