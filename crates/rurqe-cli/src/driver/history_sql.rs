//! The SQL that records the migration lifecycle, shared by both backends.
//!
//! Every backend writes the same `__rurqe_schema_history` rows, so the statement
//! text lives in one place instead of being repeated per engine. Only the
//! placeholder sigil differs: SQLite binds anonymously (`?1`) while PostgreSQL
//! numbers its parameters (`$1`), which is why each generator takes the sigil as
//! an argument.

use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{SchemaHistoryEntry, SCHEMA_HISTORY_TABLE};
use rurqe_migrate::migration::MigrationKind;
use rurqe_migrate::version::MigrationVersion;

/// Number of columns written by [`SchemaHistoryEntry`].
pub(super) const HISTORY_COLUMN_COUNT: usize = 9;

/// Savepoint wrapping one migration step inside the SQLite run lock.
pub(super) const SQLITE_STEP_SAVEPOINT: &str = "rurqe_migration_step";

/// A raw `__rurqe_schema_history` row before decoding into domain types.
pub(super) struct RawHistoryRow {
    pub(super) installed_rank: i64,
    pub(super) version: Option<String>,
    pub(super) description: String,
    pub(super) kind: String,
    pub(super) script: String,
    pub(super) checksum: Option<String>,
    pub(super) installed_on: String,
    pub(super) execution_time_ms: i64,
    pub(super) success: bool,
}

impl RawHistoryRow {
    /// Decodes the raw row into the domain entry, rejecting an unknown kind.
    pub(super) fn decode(self) -> Result<SchemaHistoryEntry, Error> {
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
pub(super) fn select_history_sql() -> String {
    format!(
        "SELECT installed_rank, version, description, \"type\", script, checksum, installed_on, \
         execution_time, success FROM {SCHEMA_HISTORY_TABLE} ORDER BY installed_rank"
    )
}

/// The statement recording one history entry, binding `HISTORY_COLUMN_COUNT` values.
pub(super) fn insert_entry_sql(sigil: char) -> String {
    format!(
        "INSERT INTO {SCHEMA_HISTORY_TABLE} (installed_rank, version, description, \"type\", \
         script, checksum, installed_on, execution_time, success) VALUES ({})",
        placeholders(sigil, HISTORY_COLUMN_COUNT)
    )
}

/// The statement dropping the entry of `installed_rank`.
pub(super) fn delete_entry_sql(sigil: char) -> String {
    format!("DELETE FROM {SCHEMA_HISTORY_TABLE} WHERE installed_rank = {sigil}1")
}

/// The statement rewriting the checksum recorded for `script`.
pub(super) fn update_checksum_sql(sigil: char) -> String {
    format!("UPDATE {SCHEMA_HISTORY_TABLE} SET checksum = {sigil}1 WHERE script = {sigil}2")
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

/// Renders `count` placeholders of the given sigil, comma-separated.
fn placeholders(sigil: char, count: usize) -> String {
    (1..=count)
        .map(|index| format!("{sigil}{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Wraps a driver failure into the engine-neutral backend error.
pub(super) fn backend_error(err: impl std::fmt::Display) -> Error {
    Error::Backend(err.to_string())
}
