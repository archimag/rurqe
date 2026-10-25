//! Transactional migration runner and lifecycle operations.
//!
//! [`Migrator`] implements the core Flyway-compatible lifecycle operations:
//! - [`migrate`](Migrator::migrate): topological execution of pending versioned
//!   and modified repeatable migrations in transaction boundaries.
//! - [`baseline`](Migrator::baseline): marks an existing schema state as a baseline
//!   without re-running prior migrations.
//! - [`repair`](Migrator::repair): fixes recorded checksums against updated scripts
//!   and purges failed attempts.
//! - [`status`](Migrator::status): inspects schema state without running a
//!   migration; it takes the run lock while it ensures the metadata table exists,
//!   exactly like the other lifecycle operations.
//! - [`with_checkpoint`](Migrator::with_checkpoint): attaches a pruning boundary
//!   that rejects targets older than the pruned history and verifies the live
//!   catalog against the anchored stage hash.
//!
//! The implementation is split by responsibility: the `backend` module holds the
//! contracts a driver implements, `guard` the exclusive run lock, `migrator` the
//! lifecycle itself, `apply` the application of a single script and the history
//! rows it leaves behind, `memory` the in-memory backend used by deterministic
//! runs and tests, `report` the values the operations produce, and `time` the
//! timestamp rendering of the metadata table.

mod apply;
mod backend;
mod guard;
mod memory;
mod migrator;
mod report;
mod time;

#[cfg(feature = "tokio")]
mod offload;

#[cfg(test)]
mod tests;

pub use backend::{MigrationBackend, MigrationTransaction};
pub use guard::LockGuard;
pub use memory::{InMemoryBackend, InMemoryTransaction};
pub use migrator::Migrator;
pub use report::{MigrationReport, MigrationStatus, MigrationStatusKind, RepairReport};
pub(crate) use time::current_timestamp;
