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
//!   that rejects targets older than the pruned history.

use std::collections::BTreeMap;

use crate::checkpoint::Checkpoint;
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
    /// been pruned.
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
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        self.validate_version_uniqueness(migrations)?;
        let history = guard.read_history()?;
        self.validate_history(&history, migrations)?;
        tracing::debug!(
            history_entries = history.len(),
            "validated schema history"
        );

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
                tracing::info!(script = %migration.script, "applying migration");
                let mut tx = guard.begin()?;
                let start_time = std::time::Instant::now();

                match tx.execute(&migration.sql) {
                    Ok(()) => {
                        let elapsed_ms = start_time.elapsed().as_millis() as i64;
                        let entry = SchemaHistoryEntry::applied(
                            current_rank,
                            migration,
                            current_timestamp(),
                            elapsed_ms,
                        );
                        tx.record_entry(&entry)?;
                        tx.commit()?;
                        tracing::info!(
                            script = %migration.script,
                            elapsed_ms = elapsed_ms,
                            "migration applied"
                        );

                        report.applied_scripts.push(migration.script.clone());
                        report.applied_count += 1;
                        report.total_time_ms += elapsed_ms;
                    }
                    Err(err) => {
                        let elapsed_ms = start_time.elapsed().as_millis() as i64;
                        if let Err(rollback_err) = tx.rollback() {
                            tracing::warn!(
                                script = %migration.script,
                                error = %rollback_err,
                                "failed to rollback failed migration transaction"
                            );
                        }
                        tracing::error!(
                            script = %migration.script,
                            error = %err,
                            "migration failed"
                        );

                        let failed_entry = SchemaHistoryEntry::failed(
                            current_rank,
                            migration,
                            current_timestamp(),
                            elapsed_ms,
                        );
                        match guard.begin() {
                            Ok(mut fail_tx) => {
                                if let Err(rec_err) = fail_tx.record_entry(&failed_entry) {
                                    tracing::warn!(
                                        script = %migration.script,
                                        error = %rec_err,
                                        "failed to record failed migration entry"
                                    );
                                    let _ = fail_tx.rollback();
                                } else if let Err(commit_err) = fail_tx.commit() {
                                    tracing::warn!(
                                        script = %migration.script,
                                        error = %commit_err,
                                        "failed to commit failed migration history entry"
                                    );
                                }
                            }
                            Err(begin_err) => {
                                tracing::warn!(
                                    script = %migration.script,
                                    error = %begin_err,
                                    "failed to open transaction to record failed migration"
                                );
                            }
                        }

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

        let next_rank = history
            .iter()
            .map(|e| e.installed_rank)
            .max()
            .unwrap_or(0)
            + 1;

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
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        let history = guard.read_history()?;
        let mut report = RepairReport::default();

        let migrations_by_script: BTreeMap<&str, &Migration> = migrations
            .iter()
            .map(|m| (m.script.as_str(), m))
            .collect();

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
    /// Flag indicating whether exclusive migration lock is acquired.
    pub locked: bool,
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

#[cfg(test)]
mod tests {
    use super::{civil_from_days, format_timestamp, InMemoryBackend, LockGuard, Migrator};

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
    use crate::checkpoint::Checkpoint;
    use crate::checksum::Checksum;
    use crate::error::Error;
    use crate::migration::Migration;

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
}
