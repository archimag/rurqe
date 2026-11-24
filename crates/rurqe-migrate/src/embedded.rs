//! Embedded migration collection and runner ergonomics.
//!
//! [`EmbeddedMigrations`] encapsulates a statically bundled set of migration scripts,
//! allowing migrations to run from an application binary without reading a script
//! directory at run time. The collection is assembled from `(script name, SQL text)`
//! pairs: the `embed_migrations!` procedural macro of the `rurqe-macros` crate reads
//! those pairs from a migrations directory at compile time and expands into this
//! constructor, so the text a binary carries is the text it was built from.
//!
//! A collection assembled from the full project artifacts —
//! [`EmbeddedMigrations::from_artifacts`] — carries the migration tail together
//! with the optional `checkpoint.toml` marker and the declarative stage:
//!
//! * an empty target is initialised from the declarative stage, and the recorded
//!   stage marker closes every versioned migration at or below its version;
//! * repeatable scripts carry no version, so a cold start applies them on top of
//!   the stage it just created; a repeatable script whose objects the declarative
//!   stage already declares must therefore be written idempotently;
//! * a baked checkpoint rejects a target sitting below the pruned boundary.
//!
//! Catalog snapshots are checked explicitly by tooling, not by migration runs.
//!
//! Every artifact is optional on its own: a collection that does not bake one
//! keeps the corresponding step switched off without disturbing the others.
//!
//! ```
//! use rurqe_migrate::{EmbeddedMigrations, InMemoryBackend};
//!
//! let embedded = EmbeddedMigrations::new(&[
//!     ("V1__init.sql", "CREATE TABLE users (id INT);"),
//!     ("R__view.sql", "CREATE VIEW users_v AS SELECT * FROM users;"),
//! ])
//! .expect("valid migrations");
//!
//! assert_eq!(embedded.len(), 2);
//!
//! let mut backend = InMemoryBackend::new();
//! let report = embedded.migrate(&mut backend).expect("migration succeeds");
//! assert_eq!(report.applied_count, 2);
//! ```
//!
//! ```
//! use rurqe_migrate::{EmbeddedArtifacts, EmbeddedMigrations, InMemoryBackend};
//!
//! let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
//!     tail: &[("V1__users.sql", "CREATE TABLE users (id INTEGER);")],
//!     checkpoint: None,
//!     stage: &[("users.sql", "CREATE TABLE users (id INTEGER, email TEXT);")],
//! })
//! .expect("valid artifacts");
//!
//! let mut backend = InMemoryBackend::new();
//! let report = embedded.migrate(&mut backend).expect("the database is cold started");
//! assert_eq!(
//!     report.cold_start.expect("the cold start runs").stage_scripts,
//!     vec!["users.sql"]
//! );
//! assert_eq!(backend.history.len(), 1);
//! ```
//!
use core::ops::Deref;
use std::sync::Arc;

use crate::checkpoint::Checkpoint;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::Migration;
#[cfg(feature = "tokio")]
use crate::runner::join_offloaded;
use crate::runner::{
    LockGuard, MigrationBackend, MigrationReport, MigrationStatus, Migrator, RepairReport,
};
use crate::sleeper::guard_blocking_entry;
use crate::stage::init::cold_start_guarded;
use crate::stage::{DeclarativeStage, StageMarker, StageScript};
use crate::version::MigrationVersion;

/// A collection of migrations embedded directly into application binaries.
///
/// Every lifecycle method drives a [`MigrationBackend`], and a backend performs
/// blocking I/O: the call runs on the calling thread until the database answers.
/// Conventions.md §3 forbids that on an async worker thread, so every blocking
/// lifecycle method has a Tokio-safe counterpart, available with the `tokio`
/// feature: [`migrate_async`](Self::migrate_async),
/// [`status_async`](Self::status_async), [`repair_async`](Self::repair_async), and
/// [`baseline_async`](Self::baseline_async). The three history wrappers delegate
/// to the matching [`Migrator`] wrapper, so the offload mechanics live in a single
/// place, while [`migrate_async`](Self::migrate_async) runs the collection's own
/// guard-free lifecycle core — cold start and tail — on the dedicated blocking
/// thread. Each counterpart hands the backend back together
/// with the outcome, so a failure keeps the connection usable for the follow-up
/// it demands.
///
/// The synchronous methods report such a call through `warn_if_in_tokio_runtime`
/// and keep running, which is the default behaviour. Compiling this crate with the
/// `strict-async` feature makes the same check refuse the call instead, and a
/// manual `tokio::task::spawn_blocking` around a synchronous method is refused as
/// well: a blocking-pool thread and a worker thread cannot be told apart from
/// inside the thread, so only the `*_async` wrappers can block safely there. The
/// wrappers themselves are unaffected by the feature, because they run the
/// guard-free cores on a blocking thread of the pool. Run the synchronous methods
/// on a plain (non-Tokio) thread otherwise.
///
/// A script name identifies one migration, so a collection refuses two entries
/// sharing one script name: one run would otherwise apply two different bodies
/// under a single recorded identity.
///
/// A collection assembled from the full project artifacts —
/// [`from_artifacts`](Self::from_artifacts) — carries the project checkpoint, the
/// and declarative stage, so [`migrate`](Self::migrate) initialises a target with
/// empty history from the stage and rejects a target below the pruned boundary.
///
/// Every artifact is held behind an [`Arc`], so cloning a collection — which the
/// asynchronous lifecycle does on each invocation — copies one pointer per artifact
/// instead of every script body the collection carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedMigrations {
    migrations: Arc<[Migration]>,
    checkpoint: Option<Checkpoint>,
    stage: Option<Arc<DeclarativeStage>>,
}

/// The raw artifacts a project configuration bakes into an application binary.
///
/// Every artifact is optional on its own, and a missing one switches off exactly
/// the lifecycle step it feeds: `stage` enables the cold start of an empty
/// database and `checkpoint` arms the pruning boundary. The texts are raw file
/// contents, and
/// [`EmbeddedMigrations::from_artifacts`] parses them with the same parsers the
/// rest of the crate uses, so a malformed artifact is rejected before any
/// connection is opened.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmbeddedArtifacts<'a> {
    /// The `(script name, SQL text)` pairs of the migration tail.
    pub tail: &'a [(&'a str, &'a str)],
    /// The raw text of `checkpoint.toml`.
    pub checkpoint: Option<&'a str>,
    /// The `(file name, DDL text)` pairs of the declarative stage.
    pub stage: &'a [(&'a str, &'a str)],
}

impl EmbeddedMigrations {
    /// Creates a new embedded migration set from raw `(script_name, sql_content)` pairs.
    ///
    /// Every script is parsed and its SHA-256 digest computed at construction time.
    /// Migrations are sorted using [`Migrator::sort_migrations`]: versioned migrations
    /// in ascending version order, followed by repeatable migrations sorted by script name.
    ///
    /// Two entries sharing one script name are rejected with
    /// [`Error::DuplicateScriptName`]: a single run would otherwise apply two
    /// different bodies under one recorded identity.
    pub fn new(scripts: &[(&str, &str)]) -> Result<Self, Error> {
        let mut parsed = Vec::with_capacity(scripts.len());
        for &(script, sql) in scripts {
            parsed.push(Migration::parse(script, sql)?);
        }
        Self::assemble(parsed)
    }

    /// Creates a collection from the raw artifacts of a project configuration.
    ///
    /// Every text is parsed here — the tail through [`Migration::parse`], the
    /// marker through [`Checkpoint::parse`] and the stage through [`StageScript::new`]
    /// — so a malformed artifact is rejected before any connection is opened. An
    /// artifact the caller does not bake leaves the lifecycle step it feeds
    /// switched off without disturbing the others.
    pub fn from_artifacts(artifacts: EmbeddedArtifacts<'_>) -> Result<Self, Error> {
        let mut parsed = Vec::with_capacity(artifacts.tail.len());
        for &(script, sql) in artifacts.tail {
            parsed.push(Migration::parse(script, sql)?);
        }

        let checkpoint = match artifacts.checkpoint {
            Some(text) => Some(Checkpoint::parse(text)?),
            None => None,
        };

        let stage = if artifacts.stage.is_empty() {
            None
        } else {
            let mut scripts = Vec::with_capacity(artifacts.stage.len());
            for &(name, ddl) in artifacts.stage {
                scripts.push(StageScript::new(name, ddl));
            }
            Some(DeclarativeStage::new(scripts))
        };

        Self::with_artifacts(parsed, checkpoint, stage)
    }

    /// Creates an empty embedded migration set.
    pub fn empty() -> Self {
        Self {
            migrations: Arc::from(Vec::new()),
            checkpoint: None,
            stage: None,
        }
    }

    /// Wraps an existing list of pre-parsed migrations, sorting them
    /// deterministically.
    ///
    /// Two entries sharing one script name are rejected with
    /// [`Error::DuplicateScriptName`] for the same reason [`new`](Self::new)
    /// rejects them.
    pub fn from_migrations(migrations: Vec<Migration>) -> Result<Self, Error> {
        Self::assemble(migrations)
    }

    /// Assembles a pure-tail collection: no checkpoint or stage.
    fn assemble(migrations: Vec<Migration>) -> Result<Self, Error> {
        Self::with_artifacts(migrations, None, None)
    }

    /// Orders `migrations` in place, refuses two entries sharing one script name, and
    /// attaches the artifacts the collection carries.
    ///
    /// The scripts were parsed into a list this collection owns, so they are ordered
    /// where they already sit instead of being cloned into a second list, and the
    /// ordered result is shared behind an [`Arc`] rather than duplicated per run.
    fn with_artifacts(
        migrations: Vec<Migration>,
        checkpoint: Option<Checkpoint>,
        stage: Option<DeclarativeStage>,
    ) -> Result<Self, Error> {
        let ordered = Migrator::order_scripts(migrations);
        Migrator::validate_script_uniqueness(&ordered)?;
        Ok(Self {
            migrations: Arc::from(ordered),
            checkpoint,
            stage: stage.map(Arc::new),
        })
    }

    /// Returns a slice of the parsed and sorted migrations.
    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    /// Returns the number of embedded migrations.
    pub fn len(&self) -> usize {
        self.migrations.len()
    }

    /// Whether there are any embedded migrations.
    pub fn is_empty(&self) -> bool {
        self.migrations.is_empty()
    }

    /// The checkpoint marker baked into the collection, if any.
    pub fn checkpoint(&self) -> Option<&Checkpoint> {
        self.checkpoint.as_ref()
    }

    /// The declarative stage baked into the collection, if any.
    pub fn stage(&self) -> Option<&DeclarativeStage> {
        self.stage.as_deref()
    }

    /// Converts this collection into a vector of [`Migration`]s.
    ///
    /// The returned vector owns a copy of every script body, while the collection
    /// shares the scripts it holds with every clone it hands out.
    pub fn to_vec(&self) -> Vec<Migration> {
        self.migrations.to_vec()
    }

    /// Runs the embedded lifecycle: the cold start and the migration tail.
    ///
    /// The whole lifecycle runs inside one exclusive span: the run lock is taken
    /// before the history is probed and released only when the run returns, so a
    /// concurrent lifecycle operation can neither initialise, drop, nor advance
    /// the schema at any point of the run — between the probe and the cold start,
    /// between the cold start and the tail. The metadata table is ensured once
    /// per run, and the history is read once, twice when the cold
    /// start recorded its marker.
    ///
    /// * An empty target is initialised from the baked declarative stage when the
    ///   collection carries one, exactly like [`cold_start`](crate::cold_start): the
    ///   stage marker sits at the newest version the collection declares, and it
    ///   records the ordered stage SQL digest, so the versioned tail the
    ///   stage already reflects does not run a second time; repeatable scripts
    ///   carry no version and therefore still run on top of the stage.
    /// * A baked `checkpoint.toml` marker arms [`Migrator::with_checkpoint`], so a
    ///   target sitting below the pruned boundary is rejected instead of being
    ///   advanced by a partial tail.
    ///
    /// The script checks of the run — two entries sharing one script name or one
    /// version, a metadata value wider than its column — read the migration list
    /// alone, so they run before the run takes the lock and before the cold start
    /// creates anything: a list the run cannot apply leaves the target exactly as
    /// it found it.
    ///
    /// A collection built by [`new`](Self::new) or
    /// [`from_migrations`](Self::from_migrations) carries none of those artifacts,
    /// so it behaves exactly as the plain tail it describes.
    ///
    /// The backend blocks the calling thread, so an async worker thread must hand
    /// the run to [`migrate_async`](Self::migrate_async) instead of calling this
    /// method directly (Conventions.md §3).
    pub fn migrate<B: MigrationBackend>(&self, backend: &mut B) -> Result<MigrationReport, Error> {
        guard_blocking_entry("EmbeddedMigrations::migrate")?;
        self.migrate_core(backend)
    }

    /// Guard-free core of [`migrate`](Self::migrate).
    ///
    /// The asynchronous wrapper runs it on a `spawn_blocking` thread, where the
    /// blocking-entry guard cannot tell a blocking-pool thread from a worker thread
    /// and would refuse work that is exactly where it belongs.
    fn migrate_core<B: MigrationBackend>(&self, backend: &mut B) -> Result<MigrationReport, Error> {
        let migrator = match &self.checkpoint {
            Some(checkpoint) => Migrator::with_checkpoint(checkpoint.clone()),
            None => Migrator::new(),
        };
        // The script checks read the migration list alone, so they run before the
        // run takes the lock and before any script executes — exactly where the
        // classic lifecycle of `Migrator::migrate_core` runs them. A list the run
        // cannot apply — two scripts sharing one version among them — is
        // therefore refused before an empty target is initialised from the
        // declarative stage and its marker committed: no connection work precedes
        // a failure that was decidable from the list alone.
        migrator.validate_scripts(&self.migrations)?;

        let mut report = MigrationReport::default();
        let mut cold_start_time = 0;

        // The whole lifecycle runs inside one exclusive span: the probe, the cold
        // start and the tail observe the same history under the same lock.
        let mut guard = LockGuard::acquire(backend)?;
        guard.ensure_schema_history_table()?;
        let history = guard.read_history()?;

        // An empty target is initialised from the baked stage; a target that
        // already carries history is advanced by the tail alone, so a cold start
        // never overwrites it.
        let cold_start_stage = if history.is_empty() {
            self.stage.as_deref()
        } else {
            None
        };

        let history = match cold_start_stage {
            Some(stage) => {
                let marker = self.stage_marker(stage)?;
                let cold = cold_start_guarded(&mut guard, stage, &marker, &history)?;
                cold_start_time = cold.total_time_ms;
                if cold.installed_rank.is_some() {
                    report.cold_start = Some(cold);
                }
                // The cold start recorded its own marker, so the tail classifies
                // the scripts against the history it left behind.
                guard.read_history()?
            }
            None => history,
        };

        let tail = migrator.migrate_guarded(&mut guard, &self.migrations, &history)?;

        report.applied_scripts = tail.applied_scripts;
        report.applied_count = tail.applied_count;
        report.total_time_ms = tail.total_time_ms + cold_start_time;

        Ok(report)
    }

    /// Builds the marker the cold start of this collection records.
    ///
    /// The marker sits at the newest version the collection declares, so every
    /// versioned migration at or below it is reported as applied. Repeatable
    /// scripts carry no version, so the marker never covers them: they run on top
    /// of the stage like any other pending repeatable migration.
    ///
    /// The marker records the ordered stage SQL digest, not the catalog digest
    /// of a checkpoint boundary.
    fn stage_marker(&self, stage: &DeclarativeStage) -> Result<StageMarker, Error> {
        Ok(StageMarker::new(
            self.marker_version()?,
            StageMarker::DESCRIPTION,
            stage.checksum(),
        ))
    }

    /// The version that closes every versioned migration the cold start already
    /// reflects.
    ///
    /// That is the newest tail version or the checkpoint boundary, whichever is
    /// higher, and `0` for a collection declaring neither. Repeatable migrations
    /// carry no version, so they stay outside the boundary and run after the cold
    /// start has created the stage.
    fn marker_version(&self) -> Result<MigrationVersion, Error> {
        marker_version_of(&self.migrations, self.checkpoint.as_ref())
    }

    /// Runs the embedded lifecycle on a dedicated blocking thread.
    ///
    /// Delegates to the guard-free core of [`migrate`](Self::migrate), so the whole
    /// protocol — cold start, checkpoint boundary and tail — runs off the
    /// runtime's worker threads.
    ///
    /// The backend is moved into the blocking task and handed back together with
    /// the outcome: a successful run returns it with the report, a failed run
    /// returns it with the error, and only a task that never returned hands nothing
    /// back — the backend is dropped inside it, and a panic there, which this crate
    /// never produces, surfaces as [`Error::Backend`] instead of unwinding the
    /// caller. The collection itself is shared with the task instead of copied, so a
    /// binary carrying the whole DDL of a project pays one pointer per artifact on
    /// every invocation.
    ///
    /// The task runs the guard-free core of the lifecycle, so it never trips the
    /// `strict-async` refusal: a blocking-pool thread is exactly where this work
    /// belongs.
    #[cfg(feature = "tokio")]
    pub async fn migrate_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
    ) -> (Result<MigrationReport, Error>, Option<B>) {
        let embedded = self.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let report = embedded.migrate_core(&mut backend);
            (report, backend)
        })
        .await;

        join_offloaded(joined, "migration")
    }

    /// Inspects the status of embedded migrations against the database history.
    ///
    /// The backend blocks the calling thread, so an async worker thread must hand
    /// the run to [`status_async`](Self::status_async) or to
    /// `tokio::task::spawn_blocking` instead of calling this method directly
    /// (Conventions.md §3).
    pub fn status<B: MigrationBackend>(
        &self,
        backend: &mut B,
    ) -> Result<Vec<MigrationStatus>, Error> {
        guard_blocking_entry("EmbeddedMigrations::status")?;
        Migrator::new().status_core(backend, &self.migrations)
    }

    /// Inspects the status of the embedded migrations on a dedicated blocking thread.
    ///
    /// Delegates to the shared offload entry point of [`Migrator`], which owns the
    /// mechanics, so this wrapper and the runner counterpart cannot drift apart, and
    /// hands over the scripts this collection already shares instead of copying them.
    /// The backend is moved into the blocking task and handed back with the
    /// outcome — a failed read returns it too — so the caller keeps the connection
    /// it inspected, and only a task that never returned hands nothing back. Such a
    /// panic surfaces as [`Error::Backend`] instead of unwinding the caller. Like
    /// [`migrate_async`](Self::migrate_async), the task runs the guard-free core of
    /// the lifecycle.
    #[cfg(feature = "tokio")]
    pub async fn status_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
    ) -> (Result<Vec<MigrationStatus>, Error>, Option<B>) {
        Migrator::new()
            .status_shared_async(backend, Arc::clone(&self.migrations))
            .await
    }

    /// Repairs schema history against updated embedded script checksums and purges failed entries.
    ///
    /// The backend blocks the calling thread, so an async worker thread hands the
    /// run to [`repair_async`](Self::repair_async) instead of calling this method
    /// directly (Conventions.md §3). Wrapping this call in
    /// `tokio::task::spawn_blocking` does not help: a blocking-pool thread cannot be
    /// told apart from a worker thread, so the `strict-async` guard refuses it there
    /// as well.
    pub fn repair<B: MigrationBackend>(&self, backend: &mut B) -> Result<RepairReport, Error> {
        guard_blocking_entry("EmbeddedMigrations::repair")?;
        Migrator::new().repair_core(backend, &self.migrations)
    }

    /// Repairs the schema history on a dedicated blocking thread.
    ///
    /// Delegates to the shared offload entry point of [`Migrator`], which owns the
    /// mechanics, hands over the scripts this collection already shares, and runs the
    /// guard-free core on a blocking thread, so an async worker thread
    /// never performs the blocking database I/O Conventions.md §3 forbids.
    ///
    /// The backend is moved into the blocking task and handed back together with
    /// the outcome — a failed repair returns it too — so the caller keeps the
    /// connection it repaired, and only a task that never returned hands nothing
    /// back. Such a panic — which this crate never produces — surfaces as
    /// [`Error::Backend`] instead of unwinding the caller. Under `strict-async` this
    /// is the sanctioned runtime entry point, because it drives the guard-free core
    /// rather than the guarded synchronous method.
    #[cfg(feature = "tokio")]
    pub async fn repair_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
    ) -> (Result<RepairReport, Error>, Option<B>) {
        Migrator::new()
            .repair_shared_async(backend, Arc::clone(&self.migrations))
            .await
    }

    /// Establishes a baseline for pre-existing databases or squashes legacy migrations.
    ///
    /// The backend blocks the calling thread, so an async worker thread hands the
    /// run to [`baseline_async`](Self::baseline_async) instead of calling this
    /// method directly (Conventions.md §3). Wrapping this call in
    /// `tokio::task::spawn_blocking` does not help: a blocking-pool thread cannot be
    /// told apart from a worker thread, so the `strict-async` guard refuses it there
    /// as well.
    pub fn baseline<B: MigrationBackend>(
        &self,
        backend: &mut B,
        version: MigrationVersion,
        description: &str,
    ) -> Result<SchemaHistoryEntry, Error> {
        guard_blocking_entry("EmbeddedMigrations::baseline")?;
        Migrator::new().baseline_core(backend, version, description)
    }

    /// Records a baseline for a pre-existing database on a dedicated blocking thread.
    ///
    /// Delegates to [`Migrator::baseline_async`], which owns the offload mechanics
    /// and runs the guard-free core on a blocking thread, so an async worker thread
    /// never performs the blocking database I/O Conventions.md §3 forbids.
    ///
    /// The backend is moved into the blocking task and handed back together with
    /// the outcome — a refused baseline returns it too — so the caller keeps the
    /// connection it baselined, and only a task that never returned hands nothing
    /// back. Such a panic — which this crate never produces — surfaces as
    /// [`Error::Backend`] instead of unwinding the caller. Under `strict-async` this
    /// is the sanctioned runtime entry point, because it drives the guard-free core
    /// rather than the guarded synchronous method.
    #[cfg(feature = "tokio")]
    pub async fn baseline_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
        version: MigrationVersion,
        description: &str,
    ) -> (Result<SchemaHistoryEntry, Error>, Option<B>) {
        Migrator::new()
            .baseline_async(backend, version, description)
            .await
    }
}

/// The version the cold-start marker of a run lands at.
///
/// That is the newest versioned migration or the pruned boundary, whichever is
/// higher, and `0` for a collection declaring neither: repeatable scripts carry
/// no version, so they stay outside the boundary.
fn marker_version_of(
    migrations: &[Migration],
    checkpoint: Option<&Checkpoint>,
) -> Result<MigrationVersion, Error> {
    let tail = migrations
        .iter()
        .filter_map(|migration| migration.version.as_ref())
        .max()
        .cloned();
    let boundary = checkpoint.map(|checkpoint| checkpoint.pruned_up_to_version().clone());

    match tail.max(boundary) {
        Some(version) => Ok(version),
        None => "0".parse(),
    }
}

impl Deref for EmbeddedMigrations {
    type Target = [Migration];

    fn deref(&self) -> &Self::Target {
        &self.migrations
    }
}

impl AsRef<[Migration]> for EmbeddedMigrations {
    fn as_ref(&self) -> &[Migration] {
        &self.migrations
    }
}

impl IntoIterator for EmbeddedMigrations {
    type Item = Migration;
    type IntoIter = std::vec::IntoIter<Migration>;

    /// Consuming the collection by value materialises its scripts, which the
    /// collection shares with every clone it hands out.
    fn into_iter(self) -> Self::IntoIter {
        Vec::from(&*self.migrations).into_iter()
    }
}

impl<'a> IntoIterator for &'a EmbeddedMigrations {
    type Item = &'a Migration;
    type IntoIter = core::slice::Iter<'a, Migration>;

    fn into_iter(self) -> Self::IntoIter {
        self.migrations.iter()
    }
}
#[cfg(test)]
mod tests;
