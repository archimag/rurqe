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
//! with the optional `checkpoint.toml` marker, the declarative stage, and the
//! `schema.lock` snapshot, so the run itself guards the database:
//!
//! * an empty target is initialised from the declarative stage, and the recorded
//!   stage marker closes every versioned migration at or below its version;
//! * repeatable scripts carry no version, so a cold start applies them on top of
//!   the stage it just created; a repeatable script whose objects the declarative
//!   stage already declares must therefore be written idempotently;
//! * a baked checkpoint rejects a target sitting below the pruned boundary;
//! * a baked snapshot anchors the digest the catalog the run produced is compared
//!   against once every script has been applied.
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
//! use rurqe_migrate::{EmbeddedArtifacts, EmbeddedMigrations, InMemoryBackend, SchemaCatalog};
//!
//! let catalog = SchemaCatalog::default();
//! let snapshot = catalog.to_lock();
//!
//! let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
//!     tail: &[("V1__users.sql", "CREATE TABLE users (id INTEGER);")],
//!     checkpoint: None,
//!     stage: &[("users.sql", "CREATE TABLE users (id INTEGER, email TEXT);")],
//!     schema_lock: Some(&snapshot),
//! })
//! .expect("valid artifacts");
//!
//! let mut backend = InMemoryBackend::new();
//! backend.catalog_checksum = Some(catalog.checksum());
//!
//! let report = embedded.migrate(&mut backend).expect("the database is cold started");
//! assert_eq!(
//!     report.cold_start.expect("the cold start runs").stage_scripts,
//!     vec!["users.sql"]
//! );
//! assert_eq!(backend.history.len(), 1);
//! ```
//!
use core::ops::Deref;

use crate::catalog::SchemaCatalog;
use crate::checkpoint::Checkpoint;
use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::Migration;
use crate::runner::{
    LockGuard, MigrationBackend, MigrationReport, MigrationStatus, Migrator, RepairReport,
};
use crate::sleeper::guard_blocking_entry;
use crate::stage::init::cold_start_core;
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
/// guard-free lifecycle core — cold start, tail, and final catalog check — on the
/// dedicated blocking thread.
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
/// declarative stage, and the digest anchored by `schema.lock` as well, so
/// [`migrate`](Self::migrate) runs the guarded lifecycle: it initialises an empty
/// target from the stage, rejects a target sitting below the pruned boundary, and
/// compares the catalog the run produced against the baked digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedMigrations {
    migrations: Vec<Migration>,
    checkpoint: Option<Checkpoint>,
    stage: Option<DeclarativeStage>,
    schema_hash: Option<Checksum>,
}

/// The raw artifacts a project configuration bakes into an application binary.
///
/// Every artifact is optional on its own, and a missing one switches off exactly
/// the lifecycle step it feeds: `stage` enables the cold start of an empty
/// database, `checkpoint` arms the pruning boundary, and `schema_lock` anchors the
/// final catalog check. The texts are the raw file contents, and
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
    /// The raw text of `schema.lock`.
    pub schema_lock: Option<&'a str>,
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
        Self::assemble(&parsed)
    }

    /// Creates a collection from the raw artifacts of a project configuration.
    ///
    /// Every text is parsed here — the tail through [`Migration::parse`], the
    /// marker through [`Checkpoint::parse`], the stage through
    /// [`StageScript::new`], and the snapshot through [`SchemaCatalog::from_lock`]
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

        let schema_hash = match artifacts.schema_lock {
            Some(text) => Some(SchemaCatalog::from_lock(text)?.checksum()),
            None => None,
        };

        Self::with_artifacts(&parsed, checkpoint, stage, schema_hash)
    }

    /// Creates an empty embedded migration set.
    pub fn empty() -> Self {
        Self {
            migrations: Vec::new(),
            checkpoint: None,
            stage: None,
            schema_hash: None,
        }
    }

    /// Wraps an existing list of pre-parsed migrations, sorting them
    /// deterministically.
    ///
    /// Two entries sharing one script name are rejected with
    /// [`Error::DuplicateScriptName`] for the same reason [`new`](Self::new)
    /// rejects them.
    pub fn from_migrations(migrations: Vec<Migration>) -> Result<Self, Error> {
        Self::assemble(&migrations)
    }

    /// Assembles a pure-tail collection: no checkpoint, stage, or snapshot.
    fn assemble(migrations: &[Migration]) -> Result<Self, Error> {
        Self::with_artifacts(migrations, None, None, None)
    }

    /// Sorts `migrations`, refuses two entries sharing one script name, and
    /// attaches the artifacts the collection carries.
    fn with_artifacts(
        migrations: &[Migration],
        checkpoint: Option<Checkpoint>,
        stage: Option<DeclarativeStage>,
        schema_hash: Option<Checksum>,
    ) -> Result<Self, Error> {
        let sorted = Migrator::sort_migrations(migrations);
        Migrator::validate_script_uniqueness(&sorted)?;
        Ok(Self {
            migrations: sorted,
            checkpoint,
            stage,
            schema_hash,
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
        self.stage.as_ref()
    }

    /// The digest anchored by the baked `schema.lock` snapshot, if any.
    pub fn schema_hash(&self) -> Option<Checksum> {
        self.schema_hash
    }

    /// Converts this collection into a vector of [`Migration`]s.
    pub fn to_vec(&self) -> Vec<Migration> {
        self.migrations.clone()
    }

    /// Runs the embedded lifecycle: the cold start, the migration tail, and the
    /// final catalog check.
    ///
    /// * An empty target is initialised from the baked declarative stage when the
    ///   collection carries one, exactly like [`cold_start`](crate::cold_start): the
    ///   stage marker sits at the newest version the collection declares and is
    ///   anchored by the baked `schema.lock` digest, so the versioned tail the
    ///   stage already reflects does not run a second time; repeatable scripts
    ///   carry no version and therefore still run on top of the stage.
    /// * A baked `checkpoint.toml` marker arms [`Migrator::with_checkpoint`], so a
    ///   target sitting below the pruned boundary is rejected instead of being
    ///   advanced by a partial tail.
    /// * A baked `schema.lock` snapshot anchors the digest the catalog the run
    ///   produced is compared against once every script has been applied; a
    ///   divergence is reported as [`Error::SchemaHashMismatch`].
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
        let mut report = MigrationReport::default();
        let mut cold_start_time = 0;

        if let Some(stage) = self.cold_start_stage(backend)? {
            let marker = self.stage_marker(stage)?;
            let cold = cold_start_core(backend, stage, &marker)?;
            cold_start_time = cold.total_time_ms;
            if cold.installed_rank.is_some() {
                report.cold_start = Some(cold);
            }
        }

        let migrator = match &self.checkpoint {
            Some(checkpoint) => Migrator::with_checkpoint(checkpoint.clone()),
            None => Migrator::new(),
        };
        let tail = migrator.migrate_core(backend, &self.migrations)?;

        report.applied_scripts = tail.applied_scripts;
        report.applied_count = tail.applied_count;
        report.total_time_ms = tail.total_time_ms + cold_start_time;

        self.verify_catalog(backend)?;

        Ok(report)
    }

    /// The stage an empty target is initialised from.
    ///
    /// `None` when the collection bakes no stage, and when the target already
    /// carries history: an initialised database is advanced by the tail alone, so a
    /// cold start never overwrites it.
    fn cold_start_stage<'a, B: MigrationBackend>(
        &'a self,
        backend: &mut B,
    ) -> Result<Option<&'a DeclarativeStage>, Error> {
        let Some(stage) = &self.stage else {
            return Ok(None);
        };
        if !history_is_empty(backend)? {
            return Ok(None);
        }
        Ok(Some(stage))
    }

    /// Builds the marker the cold start of this collection records.
    ///
    /// The marker sits at the newest version the collection declares, so every
    /// versioned migration at or below it is reported as applied, and its digest
    /// is the baked `schema.lock` one, which is exactly the digest the counterpart
    /// tail replay has to produce. A collection without a snapshot anchors the
    /// digest of its own stage instead. Repeatable scripts carry no version, so the
    /// marker never covers them: they run on top of the stage like any other
    /// pending repeatable migration.
    fn stage_marker(&self, stage: &DeclarativeStage) -> Result<StageMarker, Error> {
        let checksum = match self.schema_hash {
            Some(checksum) => checksum,
            None => stage.checksum(),
        };
        Ok(StageMarker::new(self.marker_version()?, StageMarker::DESCRIPTION, checksum))
    }

    /// The version that closes every versioned migration the cold start already
    /// reflects.
    ///
    /// That is the newest tail version or the checkpoint boundary, whichever is
    /// higher, and `0` for a collection declaring neither. Repeatable migrations
    /// carry no version, so they stay outside the boundary and run after the cold
    /// start has created the stage.
    fn marker_version(&self) -> Result<MigrationVersion, Error> {
        let tail = self
            .migrations
            .iter()
            .filter_map(|migration| migration.version.as_ref())
            .max()
            .cloned();
        let boundary = self
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.pruned_up_to_version().clone());

        match tail.max(boundary) {
            Some(version) => Ok(version),
            None => "0".parse(),
        }
    }

    /// Compares the catalog the run produced against the baked digest.
    ///
    /// A backend able to introspect itself reports the digest of the catalog it
    /// really holds; one that cannot reports nothing, which leaves the anchor
    /// unverified instead of rejecting a schema the backend never described.
    fn verify_catalog<B: MigrationBackend>(&self, backend: &mut B) -> Result<(), Error> {
        let Some(expected) = self.schema_hash else {
            return Ok(());
        };
        let Some(actual) = backend.catalog_checksum()? else {
            return Ok(());
        };
        if actual == expected {
            return Ok(());
        }

        Err(Error::SchemaHashMismatch {
            expected_hash: expected.to_hex(),
            actual_hash: actual.to_hex(),
        })
    }

    /// Runs the embedded lifecycle on a dedicated blocking thread.
    ///
    /// Delegates to the guard-free core of [`migrate`](Self::migrate), so the whole
    /// protocol — cold start, checkpoint boundary, tail, and final catalog check —
    /// runs off the runtime's worker threads.
    ///
    /// The backend is moved into the blocking task and handed back together with
    /// the report, so a connection that cannot be shared across threads stays
    /// usable after the run. A panic inside the blocking task — which this crate
    /// never produces — surfaces as [`Error::Backend`] instead of unwinding the
    /// caller.
    ///
    /// The task runs the guard-free core of the lifecycle, so it never trips the
    /// `strict-async` refusal: a blocking-pool thread is exactly where this work
    /// belongs.
    #[cfg(feature = "tokio")]
    pub async fn migrate_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
    ) -> Result<(MigrationReport, B), Error> {
        let embedded = self.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let report = embedded.migrate_core(&mut backend);
            (report, backend)
        })
        .await;

        match joined {
            Ok((report, backend)) => report.map(|report| (report, backend)),
            Err(error) => Err(Error::Backend(format!(
                "blocking migration task failed: {error}"
            ))),
        }
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
    /// Delegates to [`Migrator::status_async`], which owns the offload mechanics,
    /// so this wrapper and the runner counterpart cannot drift apart.
    ///
    /// The backend is moved into the blocking task and handed back with the
    /// statuses, so the caller keeps the connection it inspected. A panic inside
    /// the blocking task surfaces as [`Error::Backend`] instead of unwinding the
    /// caller. Like [`migrate_async`](Self::migrate_async), the task runs the
    /// guard-free core of the lifecycle.
    #[cfg(feature = "tokio")]
    pub async fn status_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
    ) -> Result<(Vec<MigrationStatus>, B), Error> {
        Migrator::new()
            .status_async(backend, &self.migrations)
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
    /// Delegates to [`Migrator::repair_async`], which owns the offload mechanics
    /// and runs the guard-free core on a blocking thread, so an async worker thread
    /// never performs the blocking database I/O Conventions.md §3 forbids.
    ///
    /// The backend is moved into the blocking task and handed back together with
    /// the report, so the caller keeps the connection it repaired. A panic inside
    /// the blocking task — which this crate never produces — surfaces as
    /// [`Error::Backend`] instead of unwinding the caller. Under `strict-async` this
    /// is the sanctioned runtime entry point, because it drives the guard-free core
    /// rather than the guarded synchronous method.
    #[cfg(feature = "tokio")]
    pub async fn repair_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
    ) -> Result<(RepairReport, B), Error> {
        Migrator::new()
            .repair_async(backend, &self.migrations)
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
    /// the recorded entry, so the caller keeps the connection it baselined. A panic
    /// inside the blocking task — which this crate never produces — surfaces as
    /// [`Error::Backend`] instead of unwinding the caller. Under `strict-async` this
    /// is the sanctioned runtime entry point, because it drives the guard-free core
    /// rather than the guarded synchronous method.
    #[cfg(feature = "tokio")]
    pub async fn baseline_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
        version: MigrationVersion,
        description: &str,
    ) -> Result<(SchemaHistoryEntry, B), Error> {
        Migrator::new()
            .baseline_async(backend, version, description)
            .await
    }
}

/// Whether the target database carries no recorded history at all.
///
/// The metadata table is created on demand, so the read is a schema write and
/// happens under the same run lock the lifecycle operations take. The guard is
/// dropped before the lifecycle continues, so the cold start that follows acquires
/// the lock on its own.
fn history_is_empty<B: MigrationBackend>(backend: &mut B) -> Result<bool, Error> {
    let mut guard = LockGuard::acquire(backend)?;
    guard.ensure_schema_history_table()?;
    Ok(guard.read_history()?.is_empty())
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

    fn into_iter(self) -> Self::IntoIter {
        self.migrations.into_iter()
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
