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
use crate::sleeper::warn_if_in_tokio_runtime;
use crate::version::MigrationVersion;

/// A collection of migrations embedded directly into application binaries.
///
/// Every lifecycle method drives a [`MigrationBackend`], and a backend performs
/// blocking I/O: the call runs on the calling thread until the database answers.
/// Conventions.md §3 forbids that on an async worker thread, so a caller already
/// running inside a Tokio runtime either uses the `*_async` wrappers (available
/// with the `tokio` feature), which move the backend onto a dedicated blocking
/// thread, or wraps the synchronous call in `tokio::task::spawn_blocking`
/// itself. The synchronous methods additionally report such a call through
/// [`warn_if_in_tokio_runtime`](crate::sleeper::warn_if_in_tokio_runtime).
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
    pub fn new(scripts: &[(&str, &str)]) -> Result<Self, Error> {
        let mut parsed = Vec::with_capacity(scripts.len());
        for &(script, sql) in scripts {
            parsed.push(Migration::parse(script, sql)?);
        }
        let sorted = Migrator::sort_migrations(&parsed);
        Ok(Self { migrations: sorted })
    }

    /// Creates an empty embedded migration set.
    pub fn empty() -> Self {
        Self {
            migrations: Vec::new(),
        }
    }

    /// Wraps an existing list of pre-parsed migrations, sorting them deterministically.
    pub fn from_migrations(migrations: Vec<Migration>) -> Self {
        let sorted = Migrator::sort_migrations(&migrations);
        Self { migrations: sorted }
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
        warn_if_in_tokio_runtime("EmbeddedMigrations::migrate");
        Migrator::new().migrate(backend, &self.migrations)
    }

    /// Runs all pending migrations on a dedicated blocking thread.
    ///
    /// The backend is moved into the blocking task and handed back together with
    /// the report, so a connection that cannot be shared across threads stays
    /// usable after the run. A panic inside the blocking task — which this crate
    /// never produces — surfaces as [`Error::Backend`] instead of unwinding the
    /// caller.
    #[cfg(feature = "tokio")]
    pub async fn migrate_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
    ) -> Result<(MigrationReport, B), Error> {
        let migrations = self.migrations.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let report = Migrator::new().migrate(&mut backend, &migrations);
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
        warn_if_in_tokio_runtime("EmbeddedMigrations::status");
        Migrator::new().status(backend, &self.migrations)
    }

    /// Inspects the status of the embedded migrations on a dedicated blocking thread.
    ///
    /// The backend is moved into the blocking task and handed back with the
    /// statuses, so the caller keeps the connection it inspected. A panic inside
    /// the blocking task surfaces as [`Error::Backend`] instead of unwinding the
    /// caller.
    #[cfg(feature = "tokio")]
    pub async fn status_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
    ) -> Result<(Vec<MigrationStatus>, B), Error> {
        let migrations = self.migrations.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let statuses = Migrator::new().status(&mut backend, &migrations);
            (statuses, backend)
        })
        .await;

        match joined {
            Ok((statuses, backend)) => statuses.map(|statuses| (statuses, backend)),
            Err(error) => Err(Error::Backend(format!(
                "blocking status task failed: {error}"
            ))),
        }
    }

    /// Repairs schema history against updated embedded script checksums and purges failed entries.
    ///
    /// The backend blocks the calling thread, so an async worker thread must wrap
    /// this call in `tokio::task::spawn_blocking` (Conventions.md §3).
    pub fn repair<B: MigrationBackend>(&self, backend: &mut B) -> Result<RepairReport, Error> {
        warn_if_in_tokio_runtime("EmbeddedMigrations::repair");
        Migrator::new().repair(backend, &self.migrations)
    }

    /// Establishes a baseline for pre-existing databases or squashes legacy migrations.
    ///
    /// The backend blocks the calling thread, so an async worker thread must wrap
    /// this call in `tokio::task::spawn_blocking` (Conventions.md §3).
    pub fn baseline<B: MigrationBackend>(
        &self,
        backend: &mut B,
        version: MigrationVersion,
        description: &str,
    ) -> Result<SchemaHistoryEntry, Error> {
        warn_if_in_tokio_runtime("EmbeddedMigrations::baseline");
        Migrator::new().baseline(backend, version, description)
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
        assert_eq!(statuses[0].status, crate::runner::MigrationStatusKind::Applied);
        assert_eq!(statuses[1].status, crate::runner::MigrationStatusKind::Applied);
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
        let embedded = EmbeddedMigrations::new(&[("V1__broken.sql", "BOOM;")])
            .expect("valid migrations");

        let error = runtime
            .block_on(embedded.migrate_async(backend))
            .expect_err("the failing script aborts the run");

        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    }
}
