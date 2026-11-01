//! Migration lifecycle backend adapters.
//!
//! [`SqliteMigrationBackend`] and [`PostgresMigrationBackend`] translate the
//! engine-neutral [`MigrationBackend`](rurqe_migrate::runner::MigrationBackend)
//! contract into driver calls, and both are implemented in the driver crates that
//! own the engines they speak to: `rurqe-sqlite` and `rurqe-postgres`, behind the
//! `migrate` feature of each. This module is the facade the command layer of the
//! toolchain reaches them through, so the protocol exists in one copy — the one the
//! drivers themselves use — while the public surface of this crate stays what it
//! was: `crate::driver::*` and the crate-root re-exports name the same two types.
//!
//! The contract each adapter fulfils keeps every step inside a driver transaction,
//! so a migration script and its `__rurqe_schema_history` row are always applied or
//! discarded together. Both adapters also report the canonical digest of the catalog
//! they hold, so a cold start anchors the very digest `schema.lock` and
//! `checkpoint.toml` carry whatever engine executed the declarative stage.

pub use rurqe_postgres::migrate::PostgresMigrationBackend;
pub use rurqe_sqlite::migrate::SqliteMigrationBackend;
