//! Transactional migration runner and lifecycle operations.
//!
//! [`Migrator`] implements the core Flyway-compatible lifecycle operations:
//! - [`migrate`](Migrator::migrate): topological execution of pending versioned
//!   and modified repeatable migrations in transaction boundaries.
//! - [`baseline`](Migrator::baseline): marks an existing schema state as a baseline
//!   without re-running prior migrations.
//! - [`repair`](Migrator::repair): fixes recorded checksums against updated scripts
//!   and purges failed attempts.
//! - [`status`](Migrator::status): inspects schema state without mutations.

use std::collections::BTreeMap;

use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use crate::migration::{Migration, MigrationKind};
use crate::version::MigrationVersion;

/// Abstract database connection capable of opening transactional scopes.
pub trait MigrationBackend {
    /// Active transaction type.
    type Transaction<'a>: MigrationTransaction
    where
        Self: 'a;

    /// Ensures the `__rurqe_schema_history` table exists.
    fn ensure_schema_history_table(&mut self) -> Result<(), Error>;

    /// Reads all entries recorded in `__rurqe_schema_history`.
    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error>;

    /// Begins a transactional boundary.
    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error>;
}

/// A transactional boundary for applying migrations and recording metadata.
pub trait MigrationTransaction {
    /// Executes a raw SQL script within the transaction.
    fn execute(&mut self, sql: &str) -> Result<(), Error>;

    /// Inserts a new entry into the schema history table.
    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error>;

    /// Deletes a recorded history entry by rank (used in repair).
    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error>;

    /// Updates the checksum of an applied migration (used in repair).
    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error>;

    /// Commits the active transaction.
    fn commit(self) -> Result<(), Error>;

    /// Rolls back the active transaction.
    fn rollback(self) -> Result<(), Error>;
}

/// Execution summary of a `migrate` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Names of scripts successfully executed.
    pub applied_scripts: Vec<String>,
    /// Number of migrations applied.
    pub applied_count: usize,
    /// Total execution time across all applied migrations in milliseconds.
    pub total_time_ms: i64,
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

/// Flyway-compatible migration lifecycle runner.
#[derive(Debug, Default, Clone)]
pub struct Migrator;

impl Migrator {
    /// Creates a new migrator instance.
    pub fn new() -> Self {
        Self
    }

    /// Sorts and queues migrations: versioned migrations in ascending version order,
    /// followed by repeatable migrations sorted by script name.
    pub fn sort_migrations(migrations: &[Migration]) -> Vec<Migration> {
        let mut versioned: Vec<Migration> = migrations
            .iter()
            .filter(|m| m.is_versioned())
            .cloned()
            .collect();
        versioned.sort_by(|a, b| a.version.cmp(&b.version));

        let mut repeatable: Vec<Migration> = migrations
            .iter()
            .filter(|m| m.is_repeatable())
            .cloned()
            .collect();
        repeatable.sort_by(|a, b| a.script.cmp(&b.script));

        let mut result = versioned;
        result.extend(repeatable);
        result
    }

    /// Runs all pending versioned and modified repeatable migrations.
    pub fn migrate<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<MigrationReport, Error> {
        backend.ensure_schema_history_table()?;
        let history = backend.read_history()?;
        self.validate_history(&history, migrations)?;

        let sorted = Self::sort_migrations(migrations);
        let baseline_version = history
            .iter()
            .filter(|e| e.kind == MigrationKind::Baseline && e.success)
            .filter_map(|e| e.version.clone())
            .max();

        let mut latest_entries_by_script: BTreeMap<&str, &SchemaHistoryEntry> = BTreeMap::new();
        for entry in &history {
            if entry.success {
                match latest_entries_by_script.get(entry.script.as_str()) {
                    Some(existing) if existing.installed_rank >= entry.installed_rank => {}
                    _ => {
                        latest_entries_by_script.insert(&entry.script, entry);
                    }
                }
            }
        }

        let mut current_rank = history
            .iter()
            .map(|e| e.installed_rank)
            .max()
            .unwrap_or(0);

        let mut report = MigrationReport::default();

        for migration in &sorted {
            let should_run = match migration.kind {
                MigrationKind::Versioned => {
                    if let Some(ref base) = baseline_version {
                        if let Some(ref v) = migration.version {
                            if v <= base {
                                false
                            } else {
                                !latest_entries_by_script.contains_key(migration.script.as_str())
                            }
                        } else {
                            false
                        }
                    } else {
                        !latest_entries_by_script.contains_key(migration.script.as_str())
                    }
                }
                MigrationKind::Repeatable => {
                    match latest_entries_by_script.get(migration.script.as_str()) {
                        None => true,
                        Some(prev) => prev.checksum != Some(migration.checksum),
                    }
                }
                MigrationKind::Baseline => false,
            };

            if should_run {
                current_rank += 1;
                let mut tx = backend.begin()?;
                let start_time = std::time::Instant::now();

                match tx.execute(&migration.sql) {
                    Ok(()) => {
                        let elapsed_ms = start_time.elapsed().as_millis() as i64;
                        let entry = SchemaHistoryEntry::applied(
                            current_rank,
                            migration,
                            "CURRENT_TIMESTAMP",
                            elapsed_ms,
                        );
                        tx.record_entry(&entry)?;
                        tx.commit()?;

                        report.applied_scripts.push(migration.script.clone());
                        report.applied_count += 1;
                        report.total_time_ms += elapsed_ms;
                    }
                    Err(err) => {
                        let _ = tx.rollback();
                        return Err(Error::MigrationExecutionFailed {
                            script: migration.script.clone(),
                            source: err.to_string(),
                        });
                    }
                }
            }
        }

        Ok(report)
    }

    /// Establishes a baseline for pre-existing databases or squashes legacy migrations.
    pub fn baseline<B: MigrationBackend>(
        &self,
        backend: &mut B,
        version: MigrationVersion,
        description: &str,
    ) -> Result<SchemaHistoryEntry, Error> {
        backend.ensure_schema_history_table()?;
        let history = backend.read_history()?;

        if let Some(existing) = history
            .iter()
            .find(|e| e.version.as_ref() == Some(&version))
        {
            return Err(Error::BaselineAlreadyExists {
                version: existing.version.as_ref().unwrap().to_string(),
            });
        }

        let next_rank = history
            .iter()
            .map(|e| e.installed_rank)
            .max()
            .unwrap_or(0)
            + 1;

        let entry = SchemaHistoryEntry::baseline(
            next_rank,
            version,
            description,
            "CURRENT_TIMESTAMP",
        );

        let mut tx = backend.begin()?;
        tx.record_entry(&entry)?;
        tx.commit()?;

        Ok(entry)
    }

    /// Repairs schema history: updates checksums of modified applied migrations
    /// and purges failed migration entries.
    pub fn repair<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<RepairReport, Error> {
        backend.ensure_schema_history_table()?;
        let history = backend.read_history()?;
        let mut report = RepairReport::default();

        let migrations_by_script: BTreeMap<&str, &Migration> = migrations
            .iter()
            .map(|m| (m.script.as_str(), m))
            .collect();

        let mut tx = backend.begin()?;

        for entry in &history {
            if !entry.success {
                tx.delete_entry(entry.installed_rank)?;
                report.removed_failed_entries += 1;
            } else if let Some(m) = migrations_by_script.get(entry.script.as_str()) {
                if m.is_versioned() && entry.is_modified(m) {
                    tx.update_checksum(&entry.script, &m.checksum)?;
                    report.updated_checksums.push(entry.script.clone());
                }
            }
        }

        tx.commit()?;
        Ok(report)
    }

    /// Inspects the status of migrations against the database history.
    pub fn status<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<Vec<MigrationStatus>, Error> {
        backend.ensure_schema_history_table()?;
        let history = backend.read_history()?;

        let baseline_version = history
            .iter()
            .filter(|e| e.kind == MigrationKind::Baseline && e.success)
            .filter_map(|e| e.version.clone())
            .max();

        let mut entries_by_script: BTreeMap<&str, &SchemaHistoryEntry> = BTreeMap::new();
        for entry in &history {
            match entries_by_script.get(entry.script.as_str()) {
                Some(existing) if existing.installed_rank >= entry.installed_rank => {}
                _ => {
                    entries_by_script.insert(&entry.script, entry);
                }
            }
        }

        let sorted = Self::sort_migrations(migrations);
        let mut statuses = Vec::new();

        for migration in sorted {
            let (status, installed_rank) = match entries_by_script.get(migration.script.as_str()) {
                Some(entry) => {
                    if !entry.success {
                        (MigrationStatusKind::Failed, Some(entry.installed_rank))
                    } else if entry.is_modified(&migration) {
                        (MigrationStatusKind::Modified, Some(entry.installed_rank))
                    } else {
                        (MigrationStatusKind::Applied, Some(entry.installed_rank))
                    }
                }
                None => {
                    if let Some(ref base) = baseline_version {
                        if let Some(ref v) = migration.version {
                            if v <= base {
                                (MigrationStatusKind::IgnoredBaseline, None)
                            } else {
                                (MigrationStatusKind::Pending, None)
                            }
                        } else {
                            (MigrationStatusKind::Pending, None)
                        }
                    } else {
                        (MigrationStatusKind::Pending, None)
                    }
                }
            };

            statuses.push(MigrationStatus {
                script: migration.script,
                version: migration.version,
                kind: migration.kind,
                status,
                installed_rank,
            });
        }

        Ok(statuses)
    }

    fn validate_history(
        &self,
        history: &[SchemaHistoryEntry],
        migrations: &[Migration],
    ) -> Result<(), Error> {
        let migrations_by_script: BTreeMap<&str, &Migration> = migrations
            .iter()
            .map(|m| (m.script.as_str(), m))
            .collect();

        for entry in history {
            if !entry.success {
                return Err(Error::FailedMigrationAlreadyApplied {
                    script: entry.script.clone(),
                    installed_rank: entry.installed_rank,
                });
            }

            if let Some(m) = migrations_by_script.get(entry.script.as_str()) {
                if m.is_versioned() && entry.is_modified(m) {
                    return Err(Error::ChecksumMismatch {
                        script: m.script.clone(),
                        recorded: entry.checksum.map(|c| c.to_hex()).unwrap_or_default(),
                        actual: m.checksum.to_hex(),
                    });
                }
            }
        }

        Ok(())
    }
}

/// An in-memory [`MigrationBackend`] for deterministic execution and testing.
#[derive(Debug, Default, Clone)]
pub struct InMemoryBackend {
    /// Recorded schema history entries.
    pub history: Vec<SchemaHistoryEntry>,
    /// Log of raw SQL executed.
    pub executed_sql: Vec<String>,
    /// Flag indicating whether table creation was requested.
    pub table_created: bool,
    /// Failure simulation on SQL execution.
    pub failure_script: Option<String>,
}

impl InMemoryBackend {
    /// Creates an empty in-memory backend.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MigrationBackend for InMemoryBackend {
    type Transaction<'a> = InMemoryTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.table_created = true;
        self.executed_sql
            .push(create_schema_history_table_sql().to_owned());
        Ok(())
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        Ok(self.history.clone())
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let initial_history = self.history.clone();
        Ok(InMemoryTransaction {
            backend: self,
            staged_history: initial_history,
            staged_sql: Vec::new(),
        })
    }
}

/// Active transaction for [`InMemoryBackend`].
pub struct InMemoryTransaction<'a> {
    backend: &'a mut InMemoryBackend,
    staged_history: Vec<SchemaHistoryEntry>,
    staged_sql: Vec<String>,
}

impl MigrationTransaction for InMemoryTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        if let Some(ref fail_sql) = self.backend.failure_script {
            if sql.contains(fail_sql) {
                return Err(Error::Backend(format!("simulated failure on `{sql}`")));
            }
        }
        self.staged_sql.push(sql.to_owned());
        Ok(())
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        self.staged_history.push(entry.clone());
        Ok(())
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        self.staged_history.retain(|e| e.installed_rank != installed_rank);
        Ok(())
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        for entry in &mut self.staged_history {
            if entry.script == script {
                entry.checksum = Some(*checksum);
            }
        }
        Ok(())
    }

    fn commit(self) -> Result<(), Error> {
        self.backend.history = self.staged_history;
        self.backend.executed_sql.extend(self.staged_sql);
        Ok(())
    }

    fn rollback(self) -> Result<(), Error> {
        Ok(())
    }
}
