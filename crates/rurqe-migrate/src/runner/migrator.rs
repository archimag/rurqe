//! The migration lifecycle runner.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::checkpoint::Checkpoint;
use crate::error::Error;
use crate::history::{validate_metadata, SchemaHistoryEntry, BASELINE_SCRIPT};
use crate::migration::{Migration, MigrationKind};
use crate::sleeper::guard_blocking_entry;
use crate::version::MigrationVersion;

use super::apply::apply_migration;
use super::backend::{with_transaction, MigrationBackend, MigrationTransaction};
use super::guard::LockGuard;
use super::report::{MigrationReport, MigrationStatus, MigrationStatusKind, RepairReport};
use super::time::current_timestamp;

/// Flyway-compatible migration lifecycle runner.
///
/// A runner created with [`Migrator::new`] executes the classic sequential
/// lifecycle. Attaching a marker with [`Migrator::with_checkpoint`] additionally
/// rejects targets whose recorded history stops below the pruned boundary.
///
/// Every lifecycle method blocks the calling thread until the database answers,
/// which Conventions.md §3 forbids on an async worker thread. A synchronous entry
/// point therefore reports an active runtime through `warn_if_in_tokio_runtime`
/// and, when this crate is compiled with the `strict-async` feature, refuses to run
/// on a Tokio-managed thread at all. Run the lifecycle on a plain (non-Tokio)
/// thread, or use an `*_async` entry point where one is available.
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
    ///
    /// The `version` and the `description` are recorded in the metadata columns of
    /// the history table, so a value wider than the column is rejected with
    /// [`Error::MetadataTooLong`] before the backend is touched.
    pub fn with_checkpoint(checkpoint: Checkpoint) -> Self {
        Self {
            checkpoint: Some(checkpoint),
        }
    }

    /// Sorts and queues migrations: versioned migrations in ascending version order,
    /// followed by repeatable migrations sorted by script name.
    ///
    /// The values are cloned because the caller asks for an owned ordering — a
    /// collection that owns the list it hands on. A lifecycle run needs the order
    /// alone and borrows it through `ordered_migrations` instead, while a collection
    /// that already owns its parsed scripts orders them in place with
    /// `order_scripts`.
    pub fn sort_migrations(migrations: &[Migration]) -> Vec<Migration> {
        Self::order_scripts(migrations.to_vec())
    }

    /// Orders `migrations` into the sequence every lifecycle run applies them in.
    ///
    /// Versioned migrations come first in ascending version order, followed by
    /// repeatable migrations sorted by script name. The list is reordered where it
    /// already sits, so a caller that parsed its own scripts — an embedded collection
    /// is one — pays no copy of a single body to establish the order. An entry that
    /// is neither versioned nor repeatable is not a script of a run: such an entry is
    /// dropped here, exactly as [`sort_migrations`](Self::sort_migrations) never
    /// returned one.
    pub(crate) fn order_scripts(mut migrations: Vec<Migration>) -> Vec<Migration> {
        migrations.retain(is_script);
        migrations.sort_by(migration_order);
        migrations
    }

    /// Orders the migrations of a run without copying their script bodies.
    ///
    /// The entries come out in exactly the order
    /// [`order_scripts`](Self::order_scripts) puts the same list in. A lifecycle run
    /// reads the order only, so it borrows the entries it was handed rather than
    /// cloning every script name, description, and SQL body on each invocation.
    fn ordered_migrations(migrations: &[Migration]) -> Vec<&Migration> {
        let mut ordered: Vec<&Migration> = migrations
            .iter()
            .filter(|migration| is_script(migration))
            .collect();
        ordered.sort_by(|left, right| migration_order(left, right));
        ordered
    }

    /// Runs all pending versioned and modified repeatable migrations.
    pub fn migrate<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<MigrationReport, Error> {
        guard_blocking_entry("Migrator::migrate")?;
        self.migrate_core(backend, migrations)
    }

    /// Guard-free core of [`migrate`](Self::migrate).
    ///
    /// The `*_async` wrappers of this crate run it on a `spawn_blocking` thread,
    /// where the blocking-entry guard cannot tell a blocking-pool thread from a
    /// worker thread and would refuse work that is exactly where it belongs.
    pub(crate) fn migrate_core<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<MigrationReport, Error> {
        self.validate_scripts(migrations)?;
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        let history = guard.read_history()?;
        self.migrate_guarded(&mut guard, migrations, &history)
    }

    /// Applies the pending tail under a run lock the caller already holds.
    ///
    /// `history` is the snapshot `guard` read, so the history validation, the
    /// catalog-hash check, and the apply loop all observe one history under one
    /// lock: a concurrent lifecycle operation can neither advance the schema
    /// between the classification and the commit of the tail nor rewrite the
    /// rows the classification read. `migrations` has already passed
    /// [`validate_scripts`](Self::validate_scripts), and `guard` is the lock the
    /// caller holds for the span it owns.
    pub(crate) fn migrate_guarded<B: MigrationBackend>(
        &self,
        guard: &mut LockGuard<'_, B>,
        migrations: &[Migration],
        history: &[SchemaHistoryEntry],
    ) -> Result<MigrationReport, Error> {
        self.validate_history(history, migrations)?;
        self.validate_catalog_hash(guard, history)?;
        tracing::debug!(history_entries = history.len(), "validated schema history");

        let sorted = Self::ordered_migrations(migrations);
        let baseline_version = history
            .iter()
            .filter(|e| e.kind == MigrationKind::Baseline && e.success)
            .filter_map(|e| e.version.clone())
            .max();

        let mut latest_entries_by_script: BTreeMap<&str, &SchemaHistoryEntry> = BTreeMap::new();
        for entry in history {
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

        for migration in sorted {
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
                let elapsed_ms = apply_migration(guard, migration, current_rank)?;

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
        guard_blocking_entry("Migrator::baseline")?;
        self.baseline_core(backend, version, description)
    }

    /// Guard-free core of [`baseline`](Self::baseline).
    pub(crate) fn baseline_core<B: MigrationBackend>(
        &self,
        backend: &mut B,
        version: MigrationVersion,
        description: &str,
    ) -> Result<SchemaHistoryEntry, Error> {
        // The version and the description are supplied by the caller, so
        // nothing bounds their length; the metadata columns of the history
        // table are bounded. The check runs before the backend is touched, so
        // an over-long value is refused with the column and the limit instead
        // of surfacing the storage error of the engine.
        validate_metadata(BASELINE_SCRIPT, Some(&version), description)?;

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

        with_transaction(&mut guard, |tx| tx.record_entry(&entry))?;

        Ok(entry)
    }

    /// Repairs schema history: updates checksums of modified applied migrations
    /// and purges failed migration entries.
    ///
    /// Two entries sharing one script name are rejected with
    /// [`Error::DuplicateScriptName`] before the backend is touched: one repair
    /// pass would otherwise rewrite the recorded checksum of an entry against an
    /// arbitrarily chosen body.
    pub fn repair<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<RepairReport, Error> {
        guard_blocking_entry("Migrator::repair")?;
        self.repair_core(backend, migrations)
    }

    /// Guard-free core of [`repair`](Self::repair).
    pub(crate) fn repair_core<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<RepairReport, Error> {
        // A script name identifies one migration, so two entries under one name
        // describe one identity with two bodies. Folding such a list into the
        // lookup below would keep an arbitrary survivor and rewrite a recorded
        // checksum against it, so the list is rejected before the backend is
        // touched — exactly as `migrate_core` rejects it.
        Self::validate_script_uniqueness(migrations)?;

        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        let history = guard.read_history()?;
        let mut report = RepairReport::default();

        let migrations_by_script: BTreeMap<&str, &Migration> =
            migrations.iter().map(|m| (m.script.as_str(), m)).collect();

        with_transaction(&mut guard, |tx| {
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

            Ok(())
        })?;

        Ok(report)
    }

    /// Inspects the status of migrations against the database history.
    ///
    /// The call applies no migration of its own, but reading the history of a
    /// database that was never initialised needs the metadata table, and creating
    /// it is a schema write. The table check and the history read therefore run
    /// under the same run lock the other lifecycle operations take, so the write
    /// cannot interleave with a concurrent [`migrate`](Self::migrate) — which on
    /// SQLite holds the write lock for the whole run — and fail spuriously.
    ///
    /// Two entries sharing one script name are rejected with
    /// [`Error::DuplicateScriptName`] before the backend is touched, because the
    /// statuses of both would otherwise be derived from one collapsed entry.
    pub fn status<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<Vec<MigrationStatus>, Error> {
        guard_blocking_entry("Migrator::status")?;
        self.status_core(backend, migrations)
    }

    /// Guard-free core of [`status`](Self::status).
    ///
    /// Ensuring the metadata table is a schema write, so the check and the
    /// history read run under the very run lock the other cores take. A `status`
    /// on a database that was never initialised therefore cannot interleave with
    /// a concurrent run — which on SQLite holds the write lock for the whole run
    /// — and fail spuriously.
    pub(crate) fn status_core<B: MigrationBackend>(
        &self,
        backend: &mut B,
        migrations: &[Migration],
    ) -> Result<Vec<MigrationStatus>, Error> {
        // Two entries sharing one script name collapse into one lookup entry, so
        // the status of both would be derived from an arbitrary survivor. The
        // list is rejected before the backend is touched, exactly as
        // `migrate_core` does.
        Self::validate_script_uniqueness(migrations)?;

        // Creating the metadata table is a schema write, so it happens under the
        // very run lock the other lifecycle operations take: a concurrent
        // [`migrate`](Self::migrate) — which on SQLite holds the write lock for
        // the whole run — can then neither interleave with the creation nor make
        // the read that follows it fail spuriously. The guard closes its scope as
        // soon as the history snapshot is taken, so the statuses are computed
        // without holding the lock.
        let history = {
            let mut guard = LockGuard::acquire(backend)?;
            guard.ensure_schema_history_table()?;
            guard.read_history()?
        };

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

        let mut statuses = Vec::new();

        for migration in Self::ordered_migrations(migrations) {
            let (status, installed_rank) = match entries_by_script.get(migration.script.as_str()) {
                Some(entry) => {
                    if !entry.success {
                        (MigrationStatusKind::Failed, Some(entry.installed_rank))
                    } else if entry.is_modified(migration) {
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
                script: migration.script.clone(),
                version: migration.version.clone(),
                kind: migration.kind,
                status,
                installed_rank,
            });
        }

        tracing::debug!(migrations = statuses.len(), "computed migration status");

        Ok(statuses)
    }

    /// Drops every object of the target database, returning the names dropped.
    ///
    /// The call is the destructive reset of a target the project owns — a
    /// dedicated test database, typically — after which the whole migration tail
    /// applies again from scratch. The recorded history goes with the other
    /// objects, so every script reads as pending on the next run, and a migration
    /// whose body was edited after being applied is applied again under its new
    /// checksum instead of being reported as modified.
    ///
    /// The call drops only: it applies no migration, rewrites no project file, and
    /// re-creates no checkpoint marker. What follows is the ordinary lifecycle —
    /// a classic project applies its versioned tail, while a project with a pruned
    /// checkpoint initialises from its declarative stage before the tail continues.
    /// The checkpoint of a marker attached with
    /// [`with_checkpoint`](Self::with_checkpoint) is neither read nor validated
    /// here: the call erases the history the boundary check reads.
    ///
    /// The objects dropped are the objects
    /// [`clean_manifest`](Self::clean_manifest) reports. The call runs under the
    /// exclusive migration lock the other lifecycle operations take, so it cannot
    /// interleave with a concurrent [`migrate`](Self::migrate), and it blocks the
    /// calling thread, which Conventions.md §3 forbids on an async worker thread.
    pub fn clean<B: MigrationBackend>(&self, backend: &mut B) -> Result<Vec<String>, Error> {
        guard_blocking_entry("Migrator::clean")?;

        let mut guard = LockGuard::acquire(backend)?;
        let dropped = guard.clean()?;
        tracing::info!(objects = dropped.len(), "cleaned the target database");

        Ok(dropped)
    }

    /// Lists the objects [`clean`](Self::clean) drops, without dropping any.
    ///
    /// The call reads the catalog alone and creates no metadata table, so unlike
    /// [`status`](Self::status) it takes no run lock: a caller can show the
    /// manifest to its operator and wait for a confirmation while a concurrent run
    /// proceeds. The names reported are exactly the names
    /// [`clean`](Self::clean) drops.
    ///
    /// The call blocks the calling thread like every other lifecycle operation.
    pub fn clean_manifest<B: MigrationBackend>(
        &self,
        backend: &mut B,
    ) -> Result<Vec<String>, Error> {
        guard_blocking_entry("Migrator::clean_manifest")?;
        backend.clean_manifest()
    }
    /// Runs every pure check over the script list of a run.
    ///
    /// The three checks read the migration list alone, so they run before the
    /// run touches the backend: a malformed run is rejected without taking the
    /// run lock and without creating the metadata table. Their order is the
    /// order of the diagnostics — a duplicated script name, then a duplicated
    /// version, then a metadata value wider than its column — so a list that
    /// breaks several rules keeps the diagnostic it reported before.
    pub(crate) fn validate_scripts(&self, migrations: &[Migration]) -> Result<(), Error> {
        Self::validate_script_uniqueness(migrations)?;
        self.validate_version_uniqueness(migrations)?;
        Self::validate_metadata_widths(migrations)?;

        Ok(())
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

    /// Rejects two migrations declaring the same script name.
    ///
    /// A script name identifies one migration, so two entries under one name
    /// would apply two different bodies under a single recorded identity: the
    /// first one is recorded, and the second is then re-applied as a modified
    /// script and recorded a second time. `EmbeddedMigrations` refuses such a
    /// set at construction; this check covers a list assembled by hand, before
    /// any script of the run executes.
    pub(crate) fn validate_script_uniqueness(migrations: &[Migration]) -> Result<(), Error> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();

        for migration in migrations {
            if !seen.insert(migration.script.as_str()) {
                return Err(Error::DuplicateScriptName {
                    script: migration.script.clone(),
                });
            }
        }

        Ok(())
    }

    /// Rejects a script whose recorded metadata does not fit the history table.
    ///
    /// [`Migration::parse`] performs the same check, so a list of parsed scripts
    /// already passed it; this gate covers a list assembled by hand, whose fields
    /// the parser never saw. It runs with the other checks — before the run takes
    /// the lock and before the metadata table is created — so a script that
    /// cannot be recorded never reaches the backend.
    fn validate_metadata_widths(migrations: &[Migration]) -> Result<(), Error> {
        for migration in migrations {
            validate_metadata(
                &migration.script,
                migration.version.as_ref(),
                &migration.description,
            )?;
        }

        Ok(())
    }
    /// Rejects a history the run cannot continue from.
    ///
    /// A failed entry is reported first, before the checkpoint boundary is
    /// measured. [`Checkpoint::validate_history`] reads that boundary through the
    /// successful rows alone, so the in-progress `<< Stage >>` marker an
    /// interrupted cold start leaves behind — an unsuccessful row carrying the
    /// boundary version — would otherwise surface as
    /// [`Error::OutdatedCheckpoint`], whose remedy is to advance the target or
    /// rebuild it from `schema/`, while `repair` is what actually clears the
    /// state. Reporting the failure first keeps the actionable diagnostic,
    /// whatever boundary is attached.
    fn validate_history(
        &self,
        history: &[SchemaHistoryEntry],
        migrations: &[Migration],
    ) -> Result<(), Error> {
        for entry in history {
            if !entry.success {
                return Err(Error::FailedMigrationAlreadyApplied {
                    script: entry.script.clone(),
                    installed_rank: entry.installed_rank,
                });
            }
        }

        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.validate_history(history)?;
        }

        let migrations_by_script: BTreeMap<&str, &Migration> =
            migrations.iter().map(|m| (m.script.as_str(), m)).collect();

        for entry in history {
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

/// Whether `migration` is a script a lifecycle run applies.
///
/// A baseline marker is synthetic: [`Migrator::baseline`] records one in the history
/// table, and no script is ever parsed into that kind, so a list assembled by hand
/// treats such an entry as absent rather than as a script without a version.
fn is_script(migration: &Migration) -> bool {
    migration.is_versioned() || migration.is_repeatable()
}

/// The order in which two scripts of one run are applied.
///
/// Versioned migrations come first in ascending version order, followed by
/// repeatable migrations sorted by script name.
fn migration_order(left: &Migration, right: &Migration) -> Ordering {
    match (left.is_versioned(), right.is_versioned()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => left.version.cmp(&right.version),
        (false, false) => left.script.cmp(&right.script),
    }
}
