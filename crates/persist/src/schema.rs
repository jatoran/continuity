//! Schema constants and migration logic.
//!
//! The schema is versioned via SQLite's `PRAGMA user_version`. Each migration
//! brings the database from `version - 1` to `version`.

use rusqlite::Connection;

use crate::Error;

/// The schema version this build of `continuity-persist` writes.
pub const CURRENT_VERSION: u32 = 8;

/// SQL for the version-1 schema. Idempotent (`IF NOT EXISTS`).
const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS buffers (
    id                  BLOB PRIMARY KEY,
    title               TEXT,
    file_path           TEXT,
    file_mtime          INTEGER,
    file_hash           BLOB,
    created_at          INTEGER NOT NULL,
    last_touched        INTEGER NOT NULL,
    deleted_at          INTEGER,
    current_snapshot_id INTEGER,
    current_revision    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS buffer_snapshots (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    buffer_id     BLOB    NOT NULL,
    revision      INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    content_blob  BLOB    NOT NULL,
    content_codec TEXT    NOT NULL DEFAULT 'zstd',
    byte_len      INTEGER NOT NULL,
    line_count    INTEGER NOT NULL,
    checksum      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_snapshots_buffer_revision
    ON buffer_snapshots(buffer_id, revision DESC);

CREATE TABLE IF NOT EXISTS buffer_edits (
    buffer_id              BLOB    NOT NULL,
    seq                    INTEGER NOT NULL,
    revision               INTEGER NOT NULL,
    ts                     INTEGER NOT NULL,
    op_kind                TEXT    NOT NULL,
    range_start_line       INTEGER,
    range_start_byte       INTEGER,
    range_end_line         INTEGER,
    range_end_byte         INTEGER,
    inserted_text          TEXT,
    removed_text           TEXT,
    selections_before_json TEXT,
    selections_after_json  TEXT,
    undo_group_id          BLOB,
    checksum_after         INTEGER NOT NULL,
    PRIMARY KEY(buffer_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_edits_buffer_revision
    ON buffer_edits(buffer_id, revision);

CREATE TABLE IF NOT EXISTS undo_groups (
    id              BLOB PRIMARY KEY,
    buffer_id       BLOB    NOT NULL,
    command_name    TEXT    NOT NULL,
    ts              INTEGER NOT NULL,
    parent_group_id BLOB
);
"#;

/// Migration to schema version 2.
///
/// Adds the `trash` table that records deleted buffers and their expiry. Per
/// spec §4 a buffer's deletion sets `buffers.deleted_at` and inserts a row
/// here carrying `expires_at = deleted_at + retention_days * 24h`.
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS trash (
    buffer_id   BLOB    PRIMARY KEY,
    deleted_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_trash_expires_at ON trash(expires_at);
"#;

/// Migration to schema version 3.
///
/// Adds the `windows` table that records per-window state for cross-session
/// restoration (Phase 14). Columns mirror spec §6 with the addition of
/// `pane_tree_json`: the recursive `PaneNode` + `Group` + `Tab` payload is
/// serialized as JSON inside the row rather than denormalized into
/// `panes`/`tabs` tables — those normalized tables are a future enhancement
/// when we need cross-window queries (e.g., "find all tabs pointing at this
/// buffer"). For Phase 14 every consumer round-trips the tree as a single
/// blob, so denormalization would be pure cost.
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS windows (
    id                    BLOB    PRIMARY KEY,
    virtual_desktop_guid  BLOB,
    monitor_id            INTEGER,
    placement_blob        BLOB,
    pane_tree_json        TEXT    NOT NULL,
    last_seen             INTEGER NOT NULL,
    deleted_at            INTEGER
);

CREATE INDEX IF NOT EXISTS idx_windows_last_seen
    ON windows(last_seen DESC);
"#;

/// Migration to schema version 4.
///
/// Adds `buffer_snapshots.label`, the optional user-supplied label for a
/// named snapshot (`buffer.mark_snapshot "<label>"`). Existing rows acquire
/// `NULL` under the `PRAGMA user_version` migration gate.
const SCHEMA_V4: &str = r#"
ALTER TABLE buffer_snapshots ADD COLUMN label TEXT;
"#;

/// Migration to schema version 5.
///
/// Adds the `closed_history` table — a single ordered stack of close
/// events spanning every window. Powers smart `tab.reopen_closed`
/// (Ctrl+Shift+T) across windows: closing a whole window pushes a
/// `kind='window'` entry carrying the full pane-tree JSON snapshot at
/// close time, so the next reopen request can reconstruct the entire
/// window even after the source window's row has been tombstoned.
///
/// `id INTEGER PRIMARY KEY AUTOINCREMENT` gives a monotonic
/// pop-newest stack ordering without trusting wall-clock — even if the
/// system clock skews, the stack walks correctly. `closed_at_ms` is
/// kept separately so the smart-reopen handler can compare against a
/// window's in-memory `recently_closed` timestamps.
const SCHEMA_V5: &str = r#"
CREATE TABLE IF NOT EXISTS closed_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    closed_at_ms    INTEGER NOT NULL,
    kind            TEXT    NOT NULL,
    window_id       BLOB,
    payload_json    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_closed_history_id_desc
    ON closed_history(id DESC);
"#;

const SCHEMA_V6: &str = r#"
ALTER TABLE buffers ADD COLUMN file_content_hash BLOB;
"#;

const SCHEMA_V7: &str = r#"
CREATE TABLE IF NOT EXISTS known_vaults (
    root_path       TEXT PRIMARY KEY COLLATE NOCASE,
    display_name    TEXT NOT NULL,
    pinned          INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    last_opened_ms  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_known_vaults_order
    ON known_vaults(pinned DESC, last_opened_ms DESC);
"#;

/// Migration to schema version 8.
///
/// Removes the discontinued typing-metrics data. The feature has no runtime
/// reader or writer, and product policy explicitly discards historical rows.
const SCHEMA_V8: &str = r#"
DROP TABLE IF EXISTS metrics_daily;
"#;

/// Apply the schema, advancing `user_version` as needed.
pub(crate) fn migrate(conn: &Connection) -> Result<(), Error> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current < 1 {
        conn.execute_batch(SCHEMA_V1)?;
    }
    if current < 2 {
        conn.execute_batch(SCHEMA_V2)?;
    }
    if current < 3 {
        conn.execute_batch(SCHEMA_V3)?;
    }
    if current < 4 {
        conn.execute_batch(SCHEMA_V4)?;
    }
    if current < 5 {
        conn.execute_batch(SCHEMA_V5)?;
    }
    if current < 6 {
        conn.execute_batch(SCHEMA_V6)?;
    }
    if current < 7 {
        conn.execute_batch(SCHEMA_V7)?;
    }
    if current < 8 {
        conn.execute_batch(SCHEMA_V8)?;
    }
    if current < CURRENT_VERSION {
        conn.pragma_update(None, "user_version", CURRENT_VERSION)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    #[test]
    fn migrate_to_v3_creates_windows_table() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let v: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_VERSION);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM trash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        let wcount: i64 = conn
            .query_row("SELECT count(*) FROM windows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wcount, 0);
    }

    #[test]
    fn migrate_adds_snapshot_label_without_metrics_table() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        // `label` column on buffer_snapshots is queryable.
        let label_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM buffer_snapshots WHERE label IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(label_count, 0);
        let metrics_table_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'metrics_daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(metrics_table_count, 0);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let v: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_VERSION);
    }

    #[test]
    fn migration_drops_legacy_metrics_table_and_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE metrics_daily (
                day_iso TEXT PRIMARY KEY,
                keystrokes INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO metrics_daily (day_iso, keystrokes) VALUES ('2026-05-13', 42);
            PRAGMA user_version = 7;",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let metrics_table_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'metrics_daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(metrics_table_count, 0);
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_VERSION);
    }

    #[test]
    fn migrate_to_v7_creates_known_vaults_table() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM known_vaults", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
