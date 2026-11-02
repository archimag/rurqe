//! Metadata rows of the schema history table.
//!
//! Every applied migration is recorded in the [`SCHEMA_HISTORY_TABLE`] table, so a
//! later run can tell which scripts are already applied, which execution failed,
//! and whether an applied script was edited after the fact — the stored
//! [`Checksum`] no longer matching the file content.

use crate::checksum::Checksum;
use crate::error::Error;
use crate::migration::{Migration, MigrationKind};
use crate::version::MigrationVersion;

/// Name of the table tracking the migration lifecycle.
pub const SCHEMA_HISTORY_TABLE: &str = "__rurqe_schema_history";

/// Width of the `version` column of the history table, in characters.
///
/// The column is declared `VARCHAR(n)`, and an engine that enforces the width
/// counts characters rather than bytes, so the validation below measures the
/// very unit the engine measures.
pub const VERSION_COLUMN_WIDTH: usize = 50;

/// Width of the `description` column of the history table, in characters.
pub const DESCRIPTION_COLUMN_WIDTH: usize = 200;

/// Width of the `script` column of the history table, in characters.
pub const SCRIPT_COLUMN_WIDTH: usize = 1000;

/// The script name recorded by a synthetic baseline entry.
pub const BASELINE_SCRIPT: &str = "<< Baseline >>";

/// DDL query to initialize the metadata history table.
pub fn create_schema_history_table_sql() -> &'static str {
    "CREATE TABLE IF NOT EXISTS __rurqe_schema_history (
    installed_rank BIGINT NOT NULL PRIMARY KEY,
    version VARCHAR(50),
    description VARCHAR(200) NOT NULL,
    type VARCHAR(20) NOT NULL,
    script VARCHAR(1000) NOT NULL,
    checksum VARCHAR(64),
    installed_on VARCHAR(50) NOT NULL,
    execution_time BIGINT NOT NULL,
    success BOOLEAN NOT NULL
);"
}

/// Verifies that a script's metadata fits the columns of the history table.
///
/// The widths are a storage contract of the metadata table: an engine that
/// enforces them — PostgreSQL — rejects an over-long value inside the
/// transaction the entry is written in, with a diagnostic that names neither
/// the script nor the limit. Every value the table records passes this check
/// first, so a script that cannot be recorded is refused while it is still a
/// name instead of in the middle of a run.
pub fn validate_metadata(
    script: &str,
    version: Option<&MigrationVersion>,
    description: &str,
) -> Result<(), Error> {
    if let Some(version) = version {
        check_width(script, "version", &version.to_string(), VERSION_COLUMN_WIDTH)?;
    }
    check_width(script, "description", description, DESCRIPTION_COLUMN_WIDTH)?;
    check_width(script, "script", script, SCRIPT_COLUMN_WIDTH)
}

/// Rejects `value` when it is wider than the column `column` declares.
fn check_width(
    script: &str,
    column: &'static str,
    value: &str,
    limit: usize,
) -> Result<(), Error> {
    let actual = value.chars().count();

    if actual <= limit {
        return Ok(());
    }

    Err(Error::MetadataTooLong {
        script: script.to_owned(),
        column,
        actual,
        limit,
    })
}
/// A single row of the [`SCHEMA_HISTORY_TABLE`] metadata table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaHistoryEntry {
    /// The position of the migration in the execution sequence.
    pub installed_rank: i64,
    /// The version of a versioned migration, `None` for a repeatable one.
    pub version: Option<MigrationVersion>,
    /// The decoded description of the script name.
    pub description: String,
    /// Whether the migration is versioned, repeatable, or a baseline marker.
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

    /// Records a synthetic baseline entry for legacy databases or squashed history.
    pub fn baseline(
        installed_rank: i64,
        version: MigrationVersion,
        description: impl Into<String>,
        installed_on: impl Into<String>,
    ) -> Self {
        Self {
            installed_rank,
            version: Some(version),
            description: description.into(),
            kind: MigrationKind::Baseline,
            script: BASELINE_SCRIPT.to_owned(),
            checksum: None,
            installed_on: installed_on.into(),
            execution_time_ms: 0,
            success: true,
        }
    }

    /// Records a failed migration attempt.
    pub fn failed(
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
            success: false,
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
    #[test]
    fn the_declared_widths_match_the_metadata_ddl() {
        let ddl = create_schema_history_table_sql();

        for (column, width) in [
            ("version", VERSION_COLUMN_WIDTH),
            ("description", DESCRIPTION_COLUMN_WIDTH),
            ("script", SCRIPT_COLUMN_WIDTH),
        ] {
            assert!(
                ddl.contains(&format!("{column} VARCHAR({width})")),
                "the DDL must declare `{column}` with the width the validation enforces: {ddl}"
            );
        }
    }

    #[test]
    fn metadata_at_the_declared_width_is_accepted() {
        let version = "1".parse::<MigrationVersion>().expect("a valid version");
        let description = "d".repeat(DESCRIPTION_COLUMN_WIDTH);
        let script = "s".repeat(SCRIPT_COLUMN_WIDTH);

        assert_eq!(
            validate_metadata(&script, Some(&version), &description),
            Ok(())
        );
    }

    #[test]
    fn a_description_beyond_the_declared_width_is_rejected() {
        let version = "1".parse::<MigrationVersion>().expect("a valid version");
        let description = "d".repeat(DESCRIPTION_COLUMN_WIDTH + 1);

        assert_eq!(
            validate_metadata("V1__init.sql", Some(&version), &description),
            Err(Error::MetadataTooLong {
                script: "V1__init.sql".to_owned(),
                column: "description",
                actual: DESCRIPTION_COLUMN_WIDTH + 1,
                limit: DESCRIPTION_COLUMN_WIDTH,
            })
        );
    }

    #[test]
    fn a_version_beyond_the_declared_width_is_rejected() {
        // Every part parses as a `u64`, so the version is valid; only its
        // rendered text outgrows the column that records it.
        let parts = vec!["1"; VERSION_COLUMN_WIDTH + 1].join("_");
        let version = parts.parse::<MigrationVersion>().expect("a valid version");
        let rendered = VERSION_COLUMN_WIDTH * 2 + 1;

        assert_eq!(version.to_string().chars().count(), rendered);
        assert_eq!(
            validate_metadata("V1__init.sql", Some(&version), "init"),
            Err(Error::MetadataTooLong {
                script: "V1__init.sql".to_owned(),
                column: "version",
                actual: rendered,
                limit: VERSION_COLUMN_WIDTH,
            })
        );
    }

    #[test]
    fn a_script_name_beyond_the_declared_width_is_rejected() {
        let script = "s".repeat(SCRIPT_COLUMN_WIDTH + 1);

        assert_eq!(
            validate_metadata(&script, None, "init"),
            Err(Error::MetadataTooLong {
                script: script.clone(),
                column: "script",
                actual: SCRIPT_COLUMN_WIDTH + 1,
                limit: SCRIPT_COLUMN_WIDTH,
            })
        );
    }
}
