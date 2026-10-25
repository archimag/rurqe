//! Contracts a database driver implements to serve the lifecycle operations.

use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;

/// Abstract database connection capable of opening transactional scopes.
pub trait MigrationBackend {
    /// Active transaction type.
    type Transaction<'a>: MigrationTransaction
    where
        Self: 'a;

    /// Ensures the `__rurqe_schema_history` table exists.
    fn ensure_schema_history_table(&mut self) -> Result<(), Error>;

    /// Reads all entries recorded in `__rurqe_schema_history`.
    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error>;

    /// Reports the canonical digest of the catalog the backend currently holds.
    ///
    /// A backend able to introspect its own schema — a live SQLite or PostgreSQL
    /// connection — reports the digest the `schema.lock` snapshot carries, so a
    /// cold start compares the marker it recorded against the catalog really
    /// present in the database. The default reports `None`, which leaves the
    /// digest of the stage marker authoritative.
    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        Ok(None)
    }

    /// Begins a transactional boundary.
    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error>;

    /// Executes `sql` outside of any transaction the runner opens.
    ///
    /// Engines reject some statements inside a transaction — `CREATE INDEX
    /// CONCURRENTLY`, `VACUUM`, `ALTER TYPE ... ADD VALUE` on older servers — so a
    /// script marked non-transactional is applied through this method. Its
    /// history row is then written by a separate transaction, so work the engine
    /// already committed is never reported as pending again.
    ///
    /// The default refuses the call with
    /// [`Error::NonTransactionalUnsupported`]: a backend that cannot run a script
    /// on its own keeps supporting transactional scripts only. The refusal is
    /// raised before anything reaches the database, so the runner names the script
    /// and returns it untouched instead of recording a failure the database never
    /// produced — a failed entry would demand a `repair` for work that never ran.
    fn execute_non_transactional(&mut self, _sql: &str) -> Result<(), Error> {
        Err(Error::NonTransactionalUnsupported { script: None })
    }

    /// Acquires an exclusive migration lock.
    fn acquire_lock(&mut self) -> Result<(), Error> {
        Ok(())
    }

    /// Releases the exclusive migration lock.
    fn release_lock(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// A transactional boundary for applying migrations and recording metadata.
pub trait MigrationTransaction {
    /// Executes a raw SQL script within the transaction.
    fn execute(&mut self, sql: &str) -> Result<(), Error>;

    /// Reports the canonical digest of the catalog the transaction sees.
    ///
    /// A cold start reads it after the stage scripts ran inside the transaction,
    /// so the marker anchors the digest of the schema the stage produced under
    /// the very engine that executes it. The default reports `None`, which keeps
    /// the digest the caller supplied.
    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        Ok(None)
    }

    /// Inserts a new entry into the schema history table.
    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error>;

    /// Deletes a recorded history entry by rank (used in repair).
    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error>;

    /// Updates the checksum of an applied migration (used in repair).
    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error>;

    /// Commits the active transaction.
    fn commit(self) -> Result<(), Error>;

    /// Rolls back the active transaction.
    fn rollback(self) -> Result<(), Error>;
}
