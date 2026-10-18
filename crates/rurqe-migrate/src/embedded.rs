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
use crate::version::MigrationVersion;

/// A collection of migrations embedded directly into application binaries.
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
    pub fn migrate<B: MigrationBackend>(
        &self,
        backend: &mut B,
    ) -> Result<MigrationReport, Error> {
        Migrator::new().migrate(backend, &self.migrations)
    }

    /// Inspects the status of embedded migrations against the database history.
    pub fn status<B: MigrationBackend>(
        &self,
        backend: &mut B,
    ) -> Result<Vec<MigrationStatus>, Error> {
        Migrator::new().status(backend, &self.migrations)
    }

    /// Repairs schema history against updated embedded script checksums and purges failed entries.
    pub fn repair<B: MigrationBackend>(
        &self,
        backend: &mut B,
    ) -> Result<RepairReport, Error> {
        Migrator::new().repair(backend, &self.migrations)
    }

    /// Establishes a baseline for pre-existing databases or squashes legacy migrations.
    pub fn baseline<B: MigrationBackend>(
        &self,
        backend: &mut B,
        version: MigrationVersion,
        description: &str,
    ) -> Result<SchemaHistoryEntry, Error> {
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
