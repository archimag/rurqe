//! The statements recording the migration lifecycle, and the row decoding.
//!
//! Every entry point of the adapter — the run that applies a script, the status
//! report, and the repair path — reads and writes the same `__rurqe_schema_history`
//! rows, so the statement text lives in one place instead of being repeated per
//! operation. The statements bind SQLite's anonymous placeholders (`?1`, `?2`, ...),
//! which is the form this driver accepts.

use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{SchemaHistoryEntry, SCHEMA_HISTORY_TABLE};
use rurqe_migrate::migration::MigrationKind;
use rurqe_migrate::version::MigrationVersion;

/// Number of columns written by [`SchemaHistoryEntry`].
pub const HISTORY_COLUMN_COUNT: usize = 9;

/// Savepoint wrapping one migration step inside the SQLite run lock.
pub(super) const SQLITE_STEP_SAVEPOINT: &str = "rurqe_migration_step";

/// The placeholder sigil every statement of this backend binds.
const SIGIL: char = '?';

/// A raw `__rurqe_schema_history` row before decoding into domain types.
///
/// The row carries what the driver read from the metadata table, so the decode path
/// can be exercised without a database and every failure it reports names the
/// recorded value that is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHistoryRow {
    /// The position of the migration in the execution sequence.
    pub installed_rank: i64,
    /// The recorded version text, `None` for a repeatable migration.
    pub version: Option<String>,
    /// The recorded description of the script.
    pub description: String,
    /// The recorded kind text.
    pub kind: String,
    /// The recorded script name.
    pub script: String,
    /// The recorded checksum, as 64 hexadecimal characters.
    pub checksum: Option<String>,
    /// The recorded installation timestamp.
    pub installed_on: String,
    /// The recorded execution time of the script in milliseconds.
    pub execution_time_ms: i64,
    /// Whether the script completed successfully.
    pub success: bool,
}

impl RawHistoryRow {
    /// Decodes the raw row into the domain entry, rejecting an unknown kind.
    pub fn decode(self) -> Result<SchemaHistoryEntry, Error> {
        Ok(SchemaHistoryEntry {
            installed_rank: self.installed_rank,
            version: match self.version {
                Some(raw) => Some(raw.parse::<MigrationVersion>()?),
                None => None,
            },
            description: self.description,
            kind: decode_kind(&self.kind)?,
            script: self.script,
            checksum: match self.checksum {
                Some(hex) => Some(Checksum::from_hex(&hex)?),
                None => None,
            },
            installed_on: self.installed_on,
            execution_time_ms: self.execution_time_ms,
            success: self.success,
        })
    }
}

/// Decodes a recorded kind name back into its [`MigrationKind`].
///
/// The stored text is the single source of the recorded kind, so a row this decoder
/// does not recognise is reported as a backend failure naming the metadata table
/// instead of being read as an arbitrary migration.
fn decode_kind(raw: &str) -> Result<MigrationKind, Error> {
    [
        MigrationKind::Versioned,
        MigrationKind::Repeatable,
        MigrationKind::Baseline,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == raw)
    .ok_or_else(|| {
        Error::Backend(format!(
            "unknown migration kind `{raw}` recorded in {SCHEMA_HISTORY_TABLE}"
        ))
    })
}

/// The statement reading the recorded history in installation order.
pub fn select_history_sql() -> String {
    format!(
        "SELECT installed_rank, version, description, \"type\", script, checksum, installed_on, \
         execution_time, success FROM {SCHEMA_HISTORY_TABLE} ORDER BY installed_rank"
    )
}

/// The statement recording one history entry, binding [`HISTORY_COLUMN_COUNT`]
/// values.
pub fn insert_entry_sql() -> String {
    format!(
        "INSERT INTO {SCHEMA_HISTORY_TABLE} (installed_rank, version, description, \"type\", \
         script, checksum, installed_on, execution_time, success) VALUES ({})",
        placeholders(HISTORY_COLUMN_COUNT)
    )
}

/// The statement dropping the entry of `installed_rank`.
pub fn delete_entry_sql() -> String {
    format!("DELETE FROM {SCHEMA_HISTORY_TABLE} WHERE installed_rank = {SIGIL}1")
}

/// The statement rewriting the checksum recorded for `script`.
pub fn update_checksum_sql() -> String {
    format!("UPDATE {SCHEMA_HISTORY_TABLE} SET checksum = {SIGIL}1 WHERE script = {SIGIL}2")
}

/// The statement releasing the savepoint wrapping one SQLite step.
pub(super) fn release_savepoint_sql() -> String {
    format!("RELEASE SAVEPOINT {SQLITE_STEP_SAVEPOINT}")
}

/// The statement rolling a SQLite step back to its savepoint and releasing it.
pub(super) fn rollback_savepoint_sql() -> String {
    format!(
        "ROLLBACK TO SAVEPOINT {SQLITE_STEP_SAVEPOINT}; RELEASE SAVEPOINT {SQLITE_STEP_SAVEPOINT}"
    )
}

/// Renders `count` placeholder sigils with their positions, comma-separated.
fn placeholders(count: usize) -> String {
    (1..=count)
        .map(|index| format!("{SIGIL}{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Wraps a driver failure into the engine-neutral backend error.
pub(super) fn backend_error(err: impl std::fmt::Display) -> Error {
    Error::Backend(err.to_string())
}