//! Metadata rows of the schema history table.
//!
//! Every applied migration is recorded in the [`SCHEMA_HISTORY_TABLE`] table, so a
//! later run can tell which scripts are already applied, which execution failed,
//! and whether an applied script was edited after the fact — the stored
//! [`Checksum`] no longer matching the file content.

use crate::checksum::Checksum;
use crate::migration::{Migration, MigrationKind};
use crate::version::MigrationVersion;

/// Name of the table tracking the migration lifecycle.
pub const SCHEMA_HISTORY_TABLE: &str = "__rurqe_schema_history";

/// A single row of the [`SCHEMA_HISTORY_TABLE`] metadata table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaHistoryEntry {
    /// The position of the migration in the execution sequence.
    pub installed_rank: i64,
    /// The version of a versioned migration, `None` for a repeatable one.
    pub version: Option<MigrationVersion>,
    /// The decoded description of the script name.
    pub description: String,
    /// Whether the migration is versioned or repeatable.
    pub kind: MigrationKind,
    /// The script name the row was recorded from.
    pub script: String,
    /// The SHA-256 digest recorded when the script was applied.
    pub checksum: Option<Checksum>,
    /// The installation timestamp, as recorded by the database.
    pub installed_on: String,
    /// The measured execution time of the script in milliseconds.
    pub execution_time_ms: i64,
    /// Whether the script completed successfully.
    pub success: bool,
}

impl SchemaHistoryEntry {
    /// Records the successful application of `migration`.
    pub fn applied(
        installed_rank: i64,
        migration: &Migration,
        installed_on: impl Into<String>,
        execution_time_ms: i64,
    ) -> Self {
        Self {
            installed_rank,
            version: migration.version.clone(),
            description: migration.description.clone(),
            kind: migration.kind,
            script: migration.script.clone(),
            checksum: Some(migration.checksum),
            installed_on: installed_on.into(),
            execution_time_ms,
            success: true,
        }
    }

    /// Whether the recorded checksum proves `migration` was edited after it was applied.
    ///
    /// Entries recorded without a checksum, such as failures, never count as
    /// modified.
    pub fn is_modified(&self, migration: &Migration) -> bool {
        self.checksum
            .is_some_and(|recorded| recorded != migration.checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migration(sql: &str) -> Migration {
        Migration::parse("V1__init.sql", sql).expect("valid script name")
    }

    #[test]
    fn applied_entry_mirrors_the_migration() {
        let original = migration("SELECT 1;");
        let entry = SchemaHistoryEntry::applied(1, &original, "2024-01-01 00:00:00", 12);

        assert_eq!(entry.installed_rank, 1);
        assert_eq!(entry.version.as_ref().unwrap().to_string(), "1");
        assert_eq!(entry.description, "init");
        assert_eq!(entry.kind, MigrationKind::Versioned);
        assert_eq!(entry.script, "V1__init.sql");
        assert_eq!(entry.checksum, Some(original.checksum));
        assert_eq!(entry.installed_on, "2024-01-01 00:00:00");
        assert_eq!(entry.execution_time_ms, 12);
        assert!(entry.success);
        assert!(!entry.is_modified(&original));
    }

    #[test]
    fn retroactive_edits_are_detected() {
        let original = migration("SELECT 1;");
        let edited = migration("SELECT 2;");
        let entry = SchemaHistoryEntry::applied(1, &original, "now", 0);

        assert!(entry.is_modified(&edited));
    }

    #[test]
    fn entries_without_a_checksum_are_never_modified() {
        let mut entry = SchemaHistoryEntry::applied(1, &migration("SELECT 1;"), "now", 0);
        entry.checksum = None;

        assert!(!entry.is_modified(&migration("SELECT 2;")));
    }
}
