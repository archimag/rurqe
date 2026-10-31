//! Embedded migration collection and runner ergonomics.
//!
//! [`EmbeddedMigrations`] encapsulates a statically bundled set of migration scripts,
//! allowing migrations to be embedded directly into application binaries via
//! [`embed_migrations!`] without runtime filesystem dependencies.
//!
//! ```
//! use rurqe_migrate::{embed_migrations, EmbeddedMigrations, InMemoryBackend};
//!
//! let embedded = embed_migrations!(
//!     "V1__init.sql" => "CREATE TABLE users (id INT);",
//!     "R__view.sql" => "CREATE VIEW users_v AS SELECT * FROM users;",
//! ).expect("valid migrations");
//!
//! assert_eq!(embedded.len(), 2);
//!
//! let mut backend = InMemoryBackend::new();
//! let report = embedded.migrate(&mut backend).expect("migration succeeds");
//! assert_eq!(report.applied_count, 2);
//! ```

use core::ops::Deref;

use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::Migration;
use crate::runner::{MigrationBackend, MigrationReport, MigrationStatus, Migrator, RepairReport};
use crate::sleeper::guard_blocking_entry;
use crate::version::MigrationVersion;

/// A collection of migrations embedded directly into application binaries.
///
/// Every lifecycle method drives a [`MigrationBackend`], and a backend performs
/// blocking I/O: the call runs on the calling thread until the database answers.
/// Conventions.md §3 forbids that on an async worker thread, so every blocking
/// lifecycle method has a Tokio-safe counterpart, available with the `tokio`
/// feature: [`migrate_async`](Self::migrate_async),
/// [`status_async`](Self::status_async), [`repair_async`](Self::repair_async), and
/// [`baseline_async`](Self::baseline_async). Each one delegates to the matching
/// [`Migrator`] wrapper, so the offload mechanics live in a single place and the
/// work runs on a dedicated blocking thread.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedMigrations {
    migrations: Vec<Migration>,
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

    /// Creates an empty embedded migration set.
    pub fn empty() -> Self {
        Self {
            migrations: Vec::new(),
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

    /// Sorts `migrations` and refuses two entries sharing one script name.
    fn assemble(migrations: &[Migration]) -> Result<Self, Error> {
        let sorted = Migrator::sort_migrations(migrations);
        Migrator::validate_script_uniqueness(&sorted)?;
        Ok(Self { migrations: sorted })
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

    /// Converts this collection into a vector of [`Migration`]s.
    pub fn to_vec(&self) -> Vec<Migration> {
        self.migrations.clone()
    }

    /// Runs all pending versioned and modified repeatable migrations against `backend`.
    ///
    /// The backend blocks the calling thread, so an async worker thread must hand
    /// the run to [`migrate_async`](Self::migrate_async) or to
    /// `tokio::task::spawn_blocking` instead of calling this method directly
    /// (Conventions.md §3).
    pub fn migrate<B: MigrationBackend>(&self, backend: &mut B) -> Result<MigrationReport, Error> {
        guard_blocking_entry("EmbeddedMigrations::migrate")?;
        Migrator::new().migrate_core(backend, &self.migrations)
    }

    /// Runs all pending migrations on a dedicated blocking thread.
    ///
    /// Delegates to [`Migrator::migrate_async`], which owns the offload mechanics,
    /// so this wrapper and the runner counterpart cannot drift apart.
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
        backend: B,
    ) -> Result<(MigrationReport, B), Error> {
        Migrator::new()
            .migrate_async(backend, &self.migrations)
            .await
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

/// Macro for embedding migration scripts directly into the application binary.
///
/// Supports either arrow notation (`"script.sql" => "sql content"`) or tuple notation
/// (`("script.sql", "sql content")`).
///
/// Returns a `Result<EmbeddedMigrations, Error>`.
///
/// ```
/// use rurqe_migrate::embed_migrations;
///
/// let embedded = embed_migrations!(
///     "V1__init.sql" => "CREATE TABLE users (id INT);",
///     "V2__orders.sql" => "CREATE TABLE orders (id INT);",
/// ).unwrap();
///
/// assert_eq!(embedded.len(), 2);
/// ```
#[macro_export]
macro_rules! embed_migrations {
    () => {
        Ok($crate::EmbeddedMigrations::empty())
    };
    ($(($script:expr, $sql:expr)),* $(,)?) => {
        $crate::EmbeddedMigrations::new(&[
            $(($script, $sql)),*
        ])
    };
    ($($script:expr => $sql:expr),* $(,)?) => {
        $crate::EmbeddedMigrations::new(&[
            $(($script, $sql)),*
        ])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the asynchronous baseline tests name `MigrationKind`, so the import
    // carries the same feature gate as the tests that read it.
    #[cfg(feature = "tokio")]
    use crate::migration::MigrationKind;
    use crate::runner::InMemoryBackend;

    fn embedded() -> EmbeddedMigrations {
        EmbeddedMigrations::new(&[
            ("V1__users.sql", "CREATE TABLE users (id INTEGER);"),
            ("V2__orders.sql", "CREATE TABLE orders (id INTEGER);"),
        ])
        .expect("valid migrations")
    }

    #[test]
    fn migrate_reports_every_applied_script() {
        let mut backend = InMemoryBackend::new();
        let report = embedded()
            .migrate(&mut backend)
            .expect("the migration run succeeds");

        assert_eq!(report.applied_count, 2);
        assert_eq!(backend.history.len(), 2);
    }

    #[test]
    fn duplicate_script_names_are_rejected_at_construction() {
        let error = EmbeddedMigrations::new(&[
            ("R__view.sql", "CREATE VIEW v AS SELECT 1;"),
            ("R__view.sql", "CREATE VIEW v AS SELECT 2;"),
        ])
        .expect_err("two scripts under one name must be rejected");

        assert_eq!(
            error,
            Error::DuplicateScriptName {
                script: "R__view.sql".to_owned()
            }
        );
    }

    #[test]
    fn from_migrations_rejects_duplicate_script_names() {
        let first = Migration::parse("R__view.sql", "SELECT 1;").expect("a valid script name");
        let second = Migration::parse("R__view.sql", "SELECT 2;").expect("a valid script name");

        let error = EmbeddedMigrations::from_migrations(vec![first, second])
            .expect_err("two scripts under one name must be rejected");

        assert_eq!(
            error,
            Error::DuplicateScriptName {
                script: "R__view.sql".to_owned()
            }
        );
    }

    #[test]
    #[cfg(feature = "tokio")]
    fn migrate_async_hands_the_backend_back() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        let (report, backend) = runtime
            .block_on(embedded().migrate_async(InMemoryBackend::new()))
            .expect("the blocking run succeeds");

        assert_eq!(report.applied_count, 2);
        assert_eq!(report.applied_scripts.len(), 2);
        assert_eq!(backend.history.len(), 2);
        assert!(backend.history.iter().all(|entry| entry.success));
    }

    #[test]
    #[cfg(feature = "tokio")]
    fn status_async_reports_the_applied_scripts() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        let embedded = embedded();
        let (_, backend) = runtime
            .block_on(embedded.migrate_async(InMemoryBackend::new()))
            .expect("the blocking run succeeds");

        let (statuses, backend) = runtime
            .block_on(embedded.status_async(backend))
            .expect("the blocking status read succeeds");

        assert_eq!(statuses.len(), 2);
        assert_eq!(
            statuses[0].status,
            crate::runner::MigrationStatusKind::Applied
        );
        assert_eq!(
            statuses[1].status,
            crate::runner::MigrationStatusKind::Applied
        );
        assert_eq!(backend.history.len(), 2);
    }

    #[test]
    #[cfg(feature = "tokio")]
    fn migrate_async_surfaces_a_backend_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        let mut backend = InMemoryBackend::new();
        backend.failure_script = Some("BOOM".to_owned());
        let embedded =
            EmbeddedMigrations::new(&[("V1__broken.sql", "BOOM;")]).expect("valid migrations");

        let error = runtime
            .block_on(embedded.migrate_async(backend))
            .expect_err("the failing script aborts the run");

        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    }

    #[test]
    #[cfg(feature = "tokio")]
    fn baseline_async_records_the_baseline_and_hands_the_backend_back() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        let version: MigrationVersion = "1.0.0".parse().expect("a valid version");

        let (entry, backend) = runtime
            .block_on(embedded().baseline_async(
                InMemoryBackend::new(),
                version.clone(),
                "legacy baseline",
            ))
            .expect("the blocking baseline succeeds");

        assert_eq!(entry.kind, MigrationKind::Baseline);
        assert_eq!(
            entry.version.as_ref().map(ToString::to_string),
            Some(version.to_string())
        );
        assert_eq!(backend.history.len(), 1);
        assert_eq!(backend.history[0].kind, MigrationKind::Baseline);
        assert!(backend.history[0].success);
    }

    #[test]
    #[cfg(feature = "tokio")]
    fn repair_async_purges_the_failed_entry_of_a_broken_script() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        // The failing run is driven on this plain test thread: the `*_async`
        // wrappers hand the backend back on success only, so the connection that
        // carries the failed entry has to come from the blocking path.
        let mut backend = InMemoryBackend::new();
        backend.failure_script = Some("BOOM".to_owned());
        let broken =
            EmbeddedMigrations::new(&[("V1__broken.sql", "BOOM;")]).expect("valid migrations");

        let error = broken
            .migrate(&mut backend)
            .expect_err("the failing script aborts the run");
        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
        assert_eq!(backend.history.len(), 1);
        assert!(!backend.history[0].success);

        backend.failure_script = None;
        let fixed =
            EmbeddedMigrations::new(&[("V1__broken.sql", "SELECT 1;")]).expect("valid migrations");

        let (report, backend) = runtime
            .block_on(fixed.repair_async(backend))
            .expect("the blocking repair succeeds");

        assert_eq!(report.removed_failed_entries, 1);
        assert!(report.updated_checksums.is_empty());
        assert!(
            backend.history.is_empty(),
            "the purge leaves a clean history behind"
        );
    }

    /// The `*_async` wrapper runs the guard-free core, so the blocking-pool thread
    /// never trips the enforcement the synchronous entry points raise.
    #[test]
    #[cfg(feature = "strict-async")]
    fn migrate_async_runs_the_guard_free_core_inside_a_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        let (report, backend) = runtime
            .block_on(embedded().migrate_async(InMemoryBackend::new()))
            .expect("the blocking run succeeds under `strict-async`");

        assert_eq!(report.applied_count, 2);
        assert_eq!(backend.history.len(), 2);
        assert!(backend.history.iter().all(|entry| entry.success));
    }

    /// The baseline wrapper drives the same guard-free core, so it stays usable
    /// from a runtime thread even when the synchronous entry points refuse to run.
    #[test]
    #[cfg(feature = "strict-async")]
    fn baseline_async_runs_the_guard_free_core_inside_a_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");

        let version: MigrationVersion = "1.0.0".parse().expect("a valid version");

        let (entry, backend) = runtime
            .block_on(embedded().baseline_async(InMemoryBackend::new(), version, "legacy baseline"))
            .expect("the blocking baseline succeeds under `strict-async`");

        assert_eq!(entry.kind, MigrationKind::Baseline);
        assert_eq!(backend.history.len(), 1);
        assert!(backend.history[0].success);
    }
}
