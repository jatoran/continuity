#![warn(missing_docs)]
//! SQLite persistence: edit log, snapshots, recovery, and hot backup.
//!
//! Owns the single SQLite connection. Lives on its own thread; never
//! reaches into core state, only consumes channel messages.

pub mod backup;
pub mod budget;
pub mod buffer_history;
pub mod buffer_listing;
pub mod checksum;
pub mod closed_history;
pub mod codec;
pub mod error;
pub mod events;
pub mod file_assoc;
pub mod handle;
pub mod handle_buffer_history;
pub mod handle_buffer_listing;
pub mod handle_metrics;
pub mod handle_snapshots;
pub mod handle_timeline;
pub mod message;
pub mod metrics;
pub mod paths;
mod persist_loop;
pub mod recover;
pub mod schema;
pub mod store;
pub mod timeline;
pub(crate) mod trace;
pub mod window_state;

pub use backup::{BackupConfig, BackupScheduler, DEFAULT_INTERVAL, DEFAULT_RETAIN};
pub use budget::{edit_row_byte_cost, snapshot_byte_cost, OVERLOAD_THRESHOLD_BYTES};
pub use buffer_history::BufferHistoryLane;
pub use buffer_listing::{BufferListFilter, BufferRecord};
pub use checksum::{fnv1a_64, fnv1a_64_chunks};
pub use closed_history::{
    peek_closed_history, pop_closed_history, push_closed_history, ClosedHistoryEntry,
    ClosedHistoryKind,
};
pub use codec::{decode_op, decode_selections, encode_edit, encode_selections};
pub use error::Error;
pub use events::{PersistEvent, PersistOperation};
pub use file_assoc::{load_active_buffer_ids, load_buffer_file, set_buffer_file, BufferFileRow};
pub use handle::{PersistClient, PersistHandle, CHANNEL_CAPACITY};
pub use message::PersistMessage;
pub use paths::{backups_dir, db_path, tutorial_seen_path};
pub use recover::{
    rebuild_buffer, rebuild_buffer_with_halt, recover_buffer, RecoveredBuffer, RecoveryHalt,
    RecoveryHaltReason,
};
pub use store::{
    EditRow, MetricsDailyDelta, MetricsDailyRow, SnapshotRow, SnapshotSummaryRow, Store,
    TopBufferRow, UndoGroupRow,
};
pub use window_state::{
    delete_window as delete_window_row, load_active_windows, save_window, WindowRow,
};
