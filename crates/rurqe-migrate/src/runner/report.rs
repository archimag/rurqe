//! Reports and statuses the lifecycle operations produce.

use crate::migration::MigrationKind;
use crate::stage::ColdStartReport;
use crate::version::MigrationVersion;

/// Execution summary of a `migrate` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Names of scripts successfully executed.
    pub applied_scripts: Vec<String>,
    /// Number of migrations applied.
    pub applied_count: usize,
    /// Total execution time across all applied migrations in milliseconds.
    pub total_time_ms: i64,
    /// The cold start this run performed, `None` when it applied the tail only.
    ///
    /// A run initialises an empty database from the embedded declarative stage
    /// before it applies the tail, so the DDL of the stage and the scripts of the
    /// tail are reported separately: `applied_scripts` never names a stage script,
    /// and the digest of the catalog the whole run produced is verified against
    /// the baked `schema.lock` snapshot once the last script has been applied.
    pub cold_start: Option<ColdStartReport>,
}

/// Summary of a `repair` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepairReport {
    /// Number of failed migration entries purged.
    pub removed_failed_entries: usize,
    /// Names of scripts whose checksums were updated.
    pub updated_checksums: Vec<String>,
}

/// State of a migration relative to the database history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatusKind {
    /// Pending execution.
    Pending,
    /// Applied successfully and matches checksum.
    Applied,
    /// Applied but script content has been modified.
    Modified,
    /// Below baseline threshold and ignored.
    IgnoredBaseline,
    /// Previously executed and failed.
    Failed,
}

/// Migration status detail item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Target migration.
    pub script: String,
    /// Migration version if versioned.
    pub version: Option<MigrationVersion>,
    /// Migration kind.
    pub kind: MigrationKind,
    /// Status relative to current schema history.
    pub status: MigrationStatusKind,
    /// Assigned or executed rank.
    pub installed_rank: Option<i64>,
}
