//! Cold start of a database from the declarative stage.
//!
//! A project following the Stage + Tail paradigm keeps the target schema as
//! plain DDL scripts in a stage directory (the [`DeclarativeStage::DIRECTORY`]
//! `schema/` by default) instead of replaying the historical migration tail.
//! Cold environments — CI runners, local development databases, container
//! spin-ups — are initialised straight from that stage: every script is applied
//! inside a single transaction and a synthetic stage marker is recorded in
//! [`SCHEMA_HISTORY_TABLE`](crate::history::SCHEMA_HISTORY_TABLE).
//!
//! The marker is a [`MigrationKind::Baseline`](crate::migration::MigrationKind::Baseline)
//! entry carrying the [`StageMarker`] version and the digest of the engine-neutral
//! [`SchemaCatalog`](crate::catalog::SchemaCatalog), so the migration runner
//! treats every versioned migration at or below the marked version as already
//! applied while the active tail above it still runs.
//!
//! A script marked non-transactional runs outside any transaction, so a stage
//! A script marked non-transactional runs outside any transaction, so a stage
//! holding one cannot be applied atomically: the cold start then commits an
//! in-progress marker before the first such script and replaces it with the
//! applied one once every script has run. An interrupted cold start therefore
//! leaves a database whose partial initialisation is reported by the next
//! attempt instead of a schema the history knows nothing about.
//!
//! A backend that cannot execute a script outside a transaction refuses it
//! before anything reaches the database. When the refused script is the first of
//! the stage, nothing was applied at all, so the committed in-progress marker is
//! withdrawn again and the refusal is reported under its own identity, naming
//! that script: the next attempt starts from a clean history instead of being
//! told an initialisation was interrupted. A stage that already applied an
//! earlier script keeps its marker, because its schema really is partially
//! initialised.
//! # Blocking entry points
//!
//! [`cold_start`] is a blocking lifecycle entry point, exactly like
//! [`Migrator::migrate`](crate::runner::Migrator::migrate),
//! [`Migrator::baseline`](crate::runner::Migrator::baseline),
//! [`Migrator::repair`](crate::runner::Migrator::repair), and
//! [`Migrator::status`](crate::runner::Migrator::status), which
//! [`EmbeddedMigrations`](crate::embedded::EmbeddedMigrations) mirrors in its
//! synchronous methods. Every one of them blocks the calling thread until the
//! backend answers, so `guard_blocking_entry` reports an active runtime before the
//! database is reached: the call warns by default and is refused when this crate
//! is compiled with the `strict-async` feature.
//!
//! `cold_start_async` (available with the `tokio` feature) is the sanctioned path
//! for an asynchronous startup: it applies the stage on a dedicated blocking
//! thread and hands the backend back together with the report, exactly like the
//! `*_async` wrappers of [`EmbeddedMigrations`](crate::embedded::EmbeddedMigrations).
//!
//! The implementation is split by responsibility: the `script` module holds the
//! DDL scripts and the stage directory, the `marker` module the synthetic history
//! entry a cold start records, and the `init` module the cold start itself
//! together with the report it produces.
//!
//! ```
//! use rurqe_migrate::{
//!     cold_start, DeclarativeStage, InMemoryBackend, SchemaCatalog, StageMarker, StageScript,
//! };
//!
//! let stage = DeclarativeStage::new(vec![StageScript::new(
//!     "users.sql",
//!     "CREATE TABLE users (id INTEGER PRIMARY KEY);",
//! )]);
//! let catalog = SchemaCatalog::default();
//! let marker = StageMarker::from_catalog("3.2".parse().expect("a valid version"), &catalog);
//!
//! let mut backend = InMemoryBackend::new();
//! let report = cold_start(&mut backend, &stage, &marker).expect("the database is cold");
//!
//! assert_eq!(report.stage_scripts, vec!["users.sql"]);
//! assert_eq!(backend.history.len(), 1);
//! ```

pub(crate) mod init;
mod marker;
mod script;

#[cfg(test)]
mod tests;

pub use init::cold_start;
#[cfg(feature = "tokio")]
pub use init::cold_start_async;
pub use init::ColdStartReport;
pub use marker::StageMarker;
pub use script::{DeclarativeStage, StageScript};

/// The synthetic script name the cold-start marker is recorded under.
pub const STAGE_SCRIPT: &str = "<< Stage >>";
