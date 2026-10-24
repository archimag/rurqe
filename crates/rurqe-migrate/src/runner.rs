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
//! - [`with_checkpoint`](Migrator::with_checkpoint): attaches a pruning boundary
//!   that rejects targets older than the pruned history and verifies the live
//!   catalog against the anchored stage hash.

use std::collections::BTreeMap;

use crate::checkpoint::Checkpoint;
use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use crate::migration::{Migration, MigrationKind};
use crate::sleeper::warn_if_in_tokio_runtime;
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

    /// Reports the canonical digest of the catalog the backend currently holds.
    ///
    /// A backend able to introspect its own schema — a live SQLite or PostgreSQL
    /// connection — reports the digest the `schema.lock` snapshot carries, so a
    /// cold start compares the marker it recorded against the catalog really
    /// present in the database. The default reports `None`, which leaves the
    /// digest of the stage marker authoritative.
    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        Ok(None)
    }

    /// Begins a transactional boundary.
    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error>;

    /// Executes `sql` outside of any transaction the runner opens.
    ///
    /// Engines reject some statements inside a transaction — `CREATE INDEX
    /// CONCURRENTLY`, `VACUUM`, `ALTER TYPE ... ADD VALUE` on older servers — so a
    /// script marked non-transactional is applied through this method. Its
    /// history row is then written by a separate transaction, so work the engine
    /// already committed is never reported as pending again.
    ///
    /// The default refuses the call with
    /// [`Error::NonTransactionalUnsupported`]: a backend that cannot run a script
    /// on its own keeps supporting transactional scripts only. The refusal is
    /// raised before anything reaches the database, so the runner names the script
    /// and returns it untouched instead of recording a failure the database never
    /// produced — a failed entry would demand a `repair` for work that never ran.
    fn execute_non_transactional(&mut self, _sql: &str) -> Result<(), Error> {
        Err(Error::NonTransactionalUnsupported { script: None })
    }

    /// Acquires an exclusive migration lock.
    fn acquire_lock(&mut self) -> Result<(), Error> {
        Ok(())
    }

    /// Releases the exclusive migration lock.
    fn release_lock(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// RAII guard managing exclusive migration locking on a [`MigrationBackend`].
pub struct LockGuard<'a, B: MigrationBackend + ?Sized> {
    backend: &'a mut B,
    active: bool,
}

impl<'a, B: MigrationBackend + ?Sized> LockGuard<'a, B> {
    /// Acquires the lock via `backend.acquire_lock()` and wraps `backend`.
    pub fn acquire(backend: &'a mut B) -> Result<Self, Error> {
        backend.acquire_lock()?;
        Ok(Self {
            backend,
            active: true,
        })
    }

    /// Convenience alias for [`acquire`](Self::acquire).
    pub fn new(backend: &'a mut B) -> Result<Self, Error> {
        Self::acquire(backend)
    }
}

impl<B: MigrationBackend + ?Sized> std::ops::Deref for LockGuard<'_, B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        self.backend
    }
}

impl<B: MigrationBackend + ?Sized> std::ops::DerefMut for LockGuard<'_, B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.backend
    }
}

impl<B: MigrationBackend + ?Sized> Drop for LockGuard<'_, B> {
    fn drop(&mut self) {
        if self.active {
            if let Err(err) = self.backend.release_lock() {
                tracing::warn!(error = %err, "failed to release migration lock");
            }
        }
    }
}

/// A transactional boundary for applying migrations and recording metadata.
pub trait MigrationTransaction {
    /// Executes a raw SQL script within the transaction.
    fn execute(&mut self, sql: &str) -> Result<(), Error>;

    /// Reports the canonical digest of the catalog the transaction sees.
    ///
    /// A cold start reads it after the stage scripts ran inside the transaction,
    /// so the marker anchors the digest of the schema the stage produced under
    /// the very engine that executes it. The default reports `None`, which keeps
    /// the digest the caller supplied.
    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        Ok(None)
    }

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
///
/// A runner created with [`Migrator::new`] executes the classic sequential
/// lifecycle. Attaching a marker with [`Migrator::with_checkpoint`] additionally
/// rejects targets whose recorded history stops below the pruned boundary.
#[derive(Debug, Default, Clone)]
pub struct Migrator {
    checkpoint: Option<Checkpoint>,
}

impl Migrator {
    /// Creates a new migrator instance without a checkpoint boundary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Anchors the runner to a [`Checkpoint`], guarding pruned history.
    ///
    /// Every migration run then verifies that the target database already sits at
    /// or above [`Checkpoint::pruned_up_to_version`]; a target below it can no
    /// longer be advanced, because the scripts that would bring it forward have
    /// been pruned. A target sitting exactly at the boundary is additionally
    /// measured against [`Checkpoint::schema_hash`], so a schema that drifted from
    /// the anchored stage is caught before the tail rewrites it.
    pub fn with_checkpoint(checkpoint: Checkpoint) -> Self {
        Self {
            checkpoint: Some(checkpoint),
        }
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
        warn_if_in_tokio_runtime("Migrator::migrate");
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        self.validate_version_uniqueness(migrations)?;
        let history = guard.read_history()?;
        self.validate_history(&history, migrations)?;
        self.validate_catalog_hash(&mut guard, &history)?;
        tracing::debug!(history_entries = history.len(), "validated schema history");

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

        let mut current_rank = history.iter().map(|e| e.installed_rank).max().unwrap_or(0);

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
                tracing::info!(script = %migration.script, "applying migration");
                let elapsed_ms = apply_migration(&mut guard, migration, current_rank)?;

                report.applied_scripts.push(migration.script.clone());
                report.applied_count += 1;
                report.total_time_ms += elapsed_ms;
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
        warn_if_in_tokio_runtime("Migrator::baseline");
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        let history = guard.read_history()?;

        if let Some(existing) = history.iter().max_by_key(|e| e.installed_rank) {
            return Err(Error::BaselineAlreadyExists {
                version: existing
                    .version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| existing.script.clone()),
            });
        }

        let next_rank = history.iter().map(|e| e.installed_rank).max().unwrap_or(0) + 1;

        tracing::info!(version = %version, description = description, "recording baseline");

        let entry =
            SchemaHistoryEntry::baseline(next_rank, version, description, current_timestamp());

        let mut tx = guard.begin()?;
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
        warn_if_in_tokio_runtime("Migrator::repair");
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        let history = guard.read_history()?;
        let mut report = RepairReport::default();

        let migrations_by_script: BTreeMap<&str, &Migration> =
            migrations.iter().map(|m| (m.script.as_str(), m)).collect();

        let mut tx = guard.begin()?;

        for entry in &history {
            if !entry.success {
                tx.delete_entry(entry.installed_rank)?;
                tracing::info!(script = %entry.script, "purging failed history entry");
                report.removed_failed_entries += 1;
            } else if let Some(m) = migrations_by_script.get(entry.script.as_str()) {
                if m.is_versioned() && entry.is_modified(m) {
                    tx.update_checksum(&entry.script, &m.checksum)?;
                    tracing::info!(script = %entry.script, "updating recorded checksum");
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
        warn_if_in_tokio_runtime("Migrator::status");
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

        tracing::debug!(migrations = statuses.len(), "computed migration status");

        Ok(statuses)
    }

    fn validate_version_uniqueness(&self, migrations: &[Migration]) -> Result<(), Error> {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();

        for migration in migrations {
            if let Some(ref version) = migration.version {
                let key = version.to_string();
                if let Some(first) = seen.get(&key) {
                    return Err(Error::DuplicateMigrationVersion {
                        version: key,
                        first: (*first).to_owned(),
                        second: migration.script.clone(),
                    });
                }
                seen.insert(key, migration.script.as_str());
            }
        }

        Ok(())
    }

    fn validate_history(
        &self,
        history: &[SchemaHistoryEntry],
        migrations: &[Migration],
    ) -> Result<(), Error> {
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.validate_history(history)?;
        }

        let migrations_by_script: BTreeMap<&str, &Migration> =
            migrations.iter().map(|m| (m.script.as_str(), m)).collect();

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

    /// Compares the live catalog of the target against the checkpoint hash.
    ///
    /// The stage marker check reads the digest recorded in the history table,
    /// which describes the schema as it was initialised. A database that drifted
    /// from that snapshot keeps the recorded digest while its real catalog moved
    /// on, so the boundary is verified against the catalog the backend reports.
    /// Only a target sitting exactly at the pruned boundary is measured: below it
    /// the run is rejected as outdated, above it the stage has already been
    /// superseded by the tail and the checkpoint no longer describes the schema.
    fn validate_catalog_hash<B: MigrationBackend>(
        &self,
        guard: &mut LockGuard<'_, B>,
        history: &[SchemaHistoryEntry],
    ) -> Result<(), Error> {
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(());
        };

        let recorded_version = history
            .iter()
            .filter(|entry| entry.success)
            .filter_map(|entry| entry.version.clone())
            .max();

        if recorded_version.as_ref() != Some(checkpoint.pruned_up_to_version()) {
            return Ok(());
        }

        // A backend that cannot introspect itself reports nothing, which leaves
        // the digest recorded in the stage marker authoritative.
        let Some(actual) = guard.catalog_checksum()? else {
            return Ok(());
        };

        if actual == checkpoint.schema_hash() {
            return Ok(());
        }

        Err(Error::CheckpointHashMismatch {
            pruned_up_to_version: checkpoint.pruned_up_to_version().to_string(),
            recorded_hash: actual.to_hex(),
            expected_hash: checkpoint.schema_hash().to_hex(),
        })
    }
}

/// Applies one migration and records its outcome in the schema history.
///
/// A transactional script runs inside the transaction the runner opens, so the
/// script and its history row commit together. A script marked non-transactional
/// is applied outside any transaction, and its history row is written afterwards
/// by a separate transaction: the engine may already have committed the
/// statement, so losing the row would report the work as pending forever.
fn apply_migration<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    migration: &Migration,
    installed_rank: i64,
) -> Result<i64, Error> {
    let start_time = std::time::Instant::now();

    if migration.is_transactional() {
        let mut tx = guard.begin()?;

        let elapsed_ms = match tx.execute(&migration.sql) {
            Ok(()) => start_time.elapsed().as_millis() as i64,
            Err(error) => {
                let elapsed_ms = start_time.elapsed().as_millis() as i64;
                if let Err(rollback_error) = tx.rollback() {
                    tracing::warn!(
                        script = %migration.script,
                        error = %rollback_error,
                        "failed to rollback failed migration transaction"
                    );
                }
                tracing::error!(
                    script = %migration.script,
                    error = %error,
                    "migration failed"
                );
                record_failure(guard, migration, installed_rank, elapsed_ms);
                return Err(execution_failed(migration, &error));
            }
        };

        let entry =
            SchemaHistoryEntry::applied(installed_rank, migration, current_timestamp(), elapsed_ms);
        tx.record_entry(&entry)?;
        tx.commit()?;
        tracing::info!(
            script = %migration.script,
            elapsed_ms = elapsed_ms,
            "migration applied"
        );

        return Ok(elapsed_ms);
    }

    let elapsed_ms = match guard.execute_non_transactional(&migration.sql) {
        Ok(()) => start_time.elapsed().as_millis() as i64,
        // The backend refused the script before it touched the database, so there
        // is no work to undo and no failure to record: a failed history entry
        // would demand a repair for a migration that never ran.
        Err(Error::NonTransactionalUnsupported { script }) => {
            return Err(Error::NonTransactionalUnsupported {
                script: Some(script.unwrap_or_else(|| migration.script.clone())),
            });
        }
        Err(error) => {
            let elapsed_ms = start_time.elapsed().as_millis() as i64;
            tracing::error!(
                script = %migration.script,
                error = %error,
                "migration failed"
            );
            record_failure(guard, migration, installed_rank, elapsed_ms);
            return Err(execution_failed(migration, &error));
        }
    };

    let entry =
        SchemaHistoryEntry::applied(installed_rank, migration, current_timestamp(), elapsed_ms);
    let mut tx = guard.begin()?;
    tx.record_entry(&entry)?;
    tx.commit()?;
    tracing::info!(
        script = %migration.script,
        elapsed_ms = elapsed_ms,
        transactional = false,
        "migration applied"
    );

    Ok(elapsed_ms)
}

/// Records a failed migration attempt, best effort.
///
/// The failure is reported to the caller already, so a backend that cannot write
/// the row must not replace the original diagnostic with a second error.
fn record_failure<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    migration: &Migration,
    installed_rank: i64,
    elapsed_ms: i64,
) {
    let entry =
        SchemaHistoryEntry::failed(installed_rank, migration, current_timestamp(), elapsed_ms);

    match guard.begin() {
        Ok(mut tx) => {
            if let Err(error) = tx.record_entry(&entry) {
                tracing::warn!(
                    script = %migration.script,
                    error = %error,
                    "failed to record failed migration entry"
                );
                let _ = tx.rollback();
            } else if let Err(error) = tx.commit() {
                tracing::warn!(
                    script = %migration.script,
                    error = %error,
                    "failed to commit failed migration history entry"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                script = %migration.script,
                error = %error,
                "failed to open transaction to record failed migration"
            );
        }
    }
}

/// Wraps `error` into the failure the caller sees for `migration`.
fn execution_failed(migration: &Migration, error: &Error) -> Error {
    Error::MigrationExecutionFailed {
        script: migration.script.clone(),
        source: error.to_string(),
    }
}

/// Renders the current UTC time as `YYYY-MM-DD HH:MM:SS`.
///
/// The history table stores `installed_on` as a plain string, so the runner has to
/// produce a concrete timestamp instead of delegating to `CURRENT_TIMESTAMP`.
/// The crate stays dependency-free, hence the epoch-to-civil conversion below.
pub(crate) fn current_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    format_timestamp(seconds)
}

/// Formats UNIX epoch seconds as a UTC `YYYY-MM-DD HH:MM:SS` timestamp.
fn format_timestamp(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Converts days since the UNIX epoch into a proleptic Gregorian civil date.
///
/// The crate keeps this private copy because it does not depend on `rurqe-core`
/// at build time. The dev-dependency on `rurqe-core` lets
/// `the_calendar_conversion_matches_the_shared_core_implementation` prove the
/// copy still agrees with `rurqe_core::calendar::civil_from_days` day by day, so
/// the two implementations cannot drift apart unnoticed.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pivot = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_pivot + 2) / 5 + 1) as u32;
    let month = if month_pivot < 10 {
        month_pivot + 3
    } else {
        month_pivot - 9
    } as u32;
    let year = if month <= 2 { year + 1 } else { year };

    (year, month, day)
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
    /// Log of the scripts executed outside a transaction.
    pub non_transactional_sql: Vec<String>,
    /// Flag indicating whether exclusive migration lock is acquired.
    pub locked: bool,
    /// Digest the backend reports for the catalog it currently holds.
    pub catalog_checksum: Option<Checksum>,
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

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        Ok(self.catalog_checksum)
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let initial_history = self.history.clone();
        Ok(InMemoryTransaction {
            backend: self,
            staged_history: initial_history,
            staged_sql: Vec::new(),
        })
    }

    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        if let Some(ref fail_sql) = self.failure_script {
            if sql.contains(fail_sql) {
                return Err(Error::Backend(format!("simulated failure on `{sql}`")));
            }
        }
        self.non_transactional_sql.push(sql.to_owned());
        Ok(())
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        if self.locked {
            return Err(Error::Backend("migration lock already acquired".to_owned()));
        }
        self.locked = true;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        self.locked = false;
        Ok(())
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
        self.staged_history
            .retain(|e| e.installed_rank != installed_rank);
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

#[cfg(test)]
mod tests {
    use super::{
        civil_from_days, format_timestamp, InMemoryBackend, InMemoryTransaction, LockGuard,
        MigrationBackend, Migrator,
    };
    use crate::checkpoint::Checkpoint;
    use crate::checksum::Checksum;
    use crate::error::Error;
    use crate::history::SchemaHistoryEntry;
    use crate::migration::{Migration, MigrationKind};

    #[test]
    fn lock_guard_manages_backend_locking_lifecycle() {
        let mut backend = InMemoryBackend::new();
        assert!(!backend.locked);
        {
            let guard = LockGuard::acquire(&mut backend).expect("lock acquired");
            assert!(guard.locked);
        }
        assert!(!backend.locked);
    }

    #[test]
    fn epoch_converts_to_the_unix_epoch_date() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00");
    }

    #[test]
    fn timestamps_convert_to_utc_civil_time() {
        assert_eq!(format_timestamp(1_700_000_000), "2023-11-14 22:13:20");
    }

    #[test]
    fn leap_days_are_accounted_for() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(format_timestamp(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn civil_dates_match_the_shared_reference_vectors() {
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(58), (1970, 2, 28));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
    }

    #[test]
    fn the_calendar_conversion_matches_the_shared_core_implementation() {
        // ±500 years of days around the epoch sweep every leap-century boundary
        // the calendar rules care about: 1500, 1600, 1700, 1800, 1900, 2000,
        // 2100, 2200, 2300 and 2400 all fall inside this range.
        for days in -182_621..=182_621_i64 {
            assert_eq!(
                civil_from_days(days),
                rurqe_core::calendar::civil_from_days(days),
                "the migration calendar diverged from the shared core one on day {days}"
            );
        }
    }

    fn boundary(version: &str) -> Checkpoint {
        Checkpoint::new(
            version.parse().expect("a valid version"),
            Checksum::of(b"stage"),
        )
    }

    #[test]
    fn checkpoint_rejects_targets_below_the_pruned_boundary() {
        let mut backend = InMemoryBackend::new();
        let init = Migration::parse("V1__init.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name");
        Migrator::new()
            .migrate(&mut backend, &[init])
            .expect("the initial run succeeds");

        let tail = Migration::parse("V2__tail.sql", "CREATE TABLE orders (id INTEGER);")
            .expect("a valid script name");
        let error = Migrator::with_checkpoint(boundary("2"))
            .migrate(&mut backend, &[tail])
            .expect_err("the boundary rejects the outdated target");

        assert!(matches!(error, Error::OutdatedCheckpoint { .. }));
        assert_eq!(backend.history.len(), 1);
    }

    #[test]
    fn checkpoint_allows_targets_at_the_pruned_boundary() {
        let mut backend = InMemoryBackend::new();
        let stage = Migration::parse("V3__stage.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name");
        Migrator::new()
            .migrate(&mut backend, &[stage])
            .expect("the initial run succeeds");

        let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
            .expect("a valid script name");
        let report = Migrator::with_checkpoint(boundary("3"))
            .migrate(&mut backend, &[tail])
            .expect("the target sits at the boundary");

        assert_eq!(report.applied_count, 1);
        assert_eq!(report.applied_scripts, vec!["V4__tail.sql"]);
    }

    #[test]
    fn checkpoint_rejects_diverged_stage_marker_hash() {
        let mut backend = InMemoryBackend::new();
        let stage_entry = SchemaHistoryEntry {
            installed_rank: 1,
            version: Some("3".parse().expect("a valid version")),
            description: "stage".to_owned(),
            kind: MigrationKind::Baseline,
            script: crate::stage::STAGE_SCRIPT.to_owned(),
            checksum: Some(Checksum::of(b"different_stage")),
            installed_on: "2024-01-01 00:00:00".to_owned(),
            execution_time_ms: 0,
            success: true,
        };
        backend.history.push(stage_entry);

        let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
            .expect("a valid script name");
        let error = Migrator::with_checkpoint(boundary("3"))
            .migrate(&mut backend, &[tail])
            .expect_err("the diverged stage marker hash must be rejected");

        assert!(matches!(error, Error::CheckpointHashMismatch { .. }));
        assert_eq!(backend.history.len(), 1);
    }

    #[test]
    fn checkpoint_accepts_a_catalog_matching_its_hash() {
        let mut backend = InMemoryBackend::new();
        let stage = Migration::parse("V3__stage.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name");
        Migrator::new()
            .migrate(&mut backend, &[stage])
            .expect("the stage migration applies");

        backend.catalog_checksum = Some(Checksum::of(b"stage"));

        let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
            .expect("a valid script name");
        let report = Migrator::with_checkpoint(boundary("3"))
            .migrate(&mut backend, &[tail])
            .expect("the live catalog matches the checkpoint hash");

        assert_eq!(report.applied_count, 1);
        assert_eq!(report.applied_scripts, vec!["V4__tail.sql"]);
    }

    #[test]
    fn checkpoint_rejects_a_catalog_that_diverged_from_its_hash() {
        let mut backend = InMemoryBackend::new();
        let stage = Migration::parse("V3__stage.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name");
        Migrator::new()
            .migrate(&mut backend, &[stage])
            .expect("the stage migration applies");

        backend.catalog_checksum = Some(Checksum::of(b"drifted"));

        let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
            .expect("a valid script name");
        let error = Migrator::with_checkpoint(boundary("3"))
            .migrate(&mut backend, &[tail])
            .expect_err("the diverged catalog stops the run");

        match &error {
            Error::CheckpointHashMismatch {
                pruned_up_to_version,
                recorded_hash,
                expected_hash,
            } => {
                assert_eq!(pruned_up_to_version, "3");
                assert_eq!(recorded_hash, &Checksum::of(b"drifted").to_hex());
                assert_eq!(expected_hash, &Checksum::of(b"stage").to_hex());
            }
            other => panic!("expected CheckpointHashMismatch, got {other:?}"),
        }
        assert_eq!(backend.history.len(), 1, "the tail never ran");
    }

    #[test]
    fn a_non_transactional_migration_runs_outside_a_transaction() {
        let mut backend = InMemoryBackend::new();
        let migration = Migration::parse(
            "V1__index.sql",
            "-- executeInTransaction=false\nCREATE INDEX CONCURRENTLY users_id ON users (id);",
        )
        .expect("a valid script name");

        let report = Migrator::new()
            .migrate(&mut backend, &[migration])
            .expect("the non-transactional script applies");

        assert_eq!(report.applied_count, 1);
        assert_eq!(backend.non_transactional_sql.len(), 1);
        assert_eq!(backend.executed_sql.len(), 1);
        assert_eq!(backend.history.len(), 1);
        assert!(backend.history[0].success);
    }

    #[test]
    fn the_history_of_a_non_transactional_migration_is_written_after_it_runs() {
        let mut backend = InMemoryBackend::new();
        let first = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name");
        let second = Migration::parse(
            "V2__index.notx.sql",
            "CREATE INDEX CONCURRENTLY users_id ON users (id);",
        )
        .expect("a valid script name");

        let report = Migrator::new()
            .migrate(&mut backend, &[first, second])
            .expect("both scripts apply");

        assert_eq!(
            report.applied_scripts,
            vec!["V1__users.sql", "V2__index.notx.sql"]
        );
        assert_eq!(backend.non_transactional_sql.len(), 1);
        assert_eq!(backend.history.len(), 2);
        assert!(backend.history.iter().all(|entry| entry.success));
    }

    #[test]
    fn a_failed_non_transactional_migration_is_recorded_as_failed() {
        let mut backend = InMemoryBackend::new();
        backend.failure_script = Some("BOOM".to_owned());
        let migration =
            Migration::parse("V1__broken.notx.sql", "BOOM;").expect("a valid script name");

        let error = Migrator::new()
            .migrate(&mut backend, &[migration])
            .expect_err("the failing script aborts the run");

        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
        assert_eq!(backend.history.len(), 1);
        assert!(!backend.history[0].success);
        assert!(!backend.locked);
    }

    /// A backend that keeps every script inside a transaction.
    ///
    /// It leaves `execute_non_transactional` at its default, so it behaves exactly
    /// like a driver that cannot commit a statement outside the runner's
    /// transaction.
    struct TransactionOnlyBackend(InMemoryBackend);

    impl MigrationBackend for TransactionOnlyBackend {
        type Transaction<'a>
            = InMemoryTransaction<'a>
        where
            Self: 'a;

        fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
            self.0.ensure_schema_history_table()
        }

        fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
            self.0.read_history()
        }

        fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
            self.0.begin()
        }
    }

    #[test]
    fn a_backend_without_non_transactional_support_names_the_refused_script() {
        let mut backend = TransactionOnlyBackend(InMemoryBackend::new());
        let first = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name");
        let index = Migration::parse(
            "V2__index.notx.sql",
            "CREATE INDEX CONCURRENTLY users_id ON users (id);",
        )
        .expect("a valid script name");

        let error = Migrator::new()
            .migrate(&mut backend, &[first, index])
            .expect_err("the backend keeps the script inside a transaction");

        match &error {
            Error::NonTransactionalUnsupported { script } => {
                assert_eq!(script.as_deref(), Some("V2__index.notx.sql"));
            }
            other => panic!("expected NonTransactionalUnsupported, got {other:?}"),
        }
        assert!(
            !error.to_string().contains("CREATE INDEX"),
            "the diagnostic never echoes the script body: {error}"
        );
        assert_eq!(
            backend.0.history.len(),
            1,
            "the refused script leaves no failed entry behind"
        );
        assert!(backend.0.history[0].success);
        assert!(
            backend.0.non_transactional_sql.is_empty(),
            "the refused script never reached the backend"
        );
        assert!(!backend.0.locked, "the lock is released on the way out");
    }
}
