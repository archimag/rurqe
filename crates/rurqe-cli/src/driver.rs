//! Migration lifecycle driver adapters.
//!
//! [`SqliteMigrationBackend`] and [`PostgresMigrationBackend`] translate the
//! engine-neutral [`MigrationBackend`](rurqe_migrate::runner::MigrationBackend)
//! contract into driver calls. Every step runs inside a driver transaction, so a
//! migration script and its `__rurqe_schema_history` row are always applied or
//! discarded together.
//!
//! Both adapters also report the canonical digest of the catalog they hold, so a
//! cold start anchors the very digest `schema.lock` and `checkpoint.toml` carry
//! whatever engine executed the declarative stage.
//!
//! The module is split by concern: `history_sql` renders the statements both
//! engines share, `sqlite` and `postgres` own their backend, and `tests` drives
//! the drivers without a live server wherever the engine allows it.

mod history_sql;
mod postgres;
mod sqlite;

#[cfg(test)]
mod tests;

pub use postgres::PostgresMigrationBackend;
pub use sqlite::SqliteMigrationBackend;
