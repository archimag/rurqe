//! Contracts a database driver implements to serve the lifecycle operations.

use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;

use super::guard::LockGuard;

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
    ///
    /// The returned transaction obeys the termination contract of
    /// [`MigrationTransaction`]: it is committed or rolled back before the runner
    /// moves on, and one abandoned without a terminal call must leave no open
    /// database transaction behind.
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

    /// Lists the objects a [`clean`](Self::clean) call drops.
    ///
    /// The list covers every user object of the target the migration lifecycle
    /// owns: the relations of the schemas in scope, the `__rurqe_schema_history`
    /// table among them. Nothing is dropped here and no metadata table is created,
    /// so a caller can show the list to its operator before the destructive call is
    /// confirmed; [`clean`](Self::clean) then drops exactly these objects and
    /// reports the same names back.
    ///
    /// The default refuses the call with [`Error::CleanUnsupported`]: a backend
    /// that cannot drop its objects supports the self-managing lifecycle only. The
    /// refusal is raised before anything reaches the database, so a refused clean
    /// leaves both the target and its recorded history untouched.
    fn clean_manifest(&mut self) -> Result<Vec<String>, Error> {
        Err(Error::CleanUnsupported)
    }

    /// Drops every object of the target database, reporting the names dropped.
    ///
    /// This is the destructive reset of a target the project owns — a dedicated
    /// test database, typically — after which the whole migration tail applies
    /// again from scratch. The recorded history is dropped with the other objects,
    /// so every script reads as pending on the next run, and a migration whose body
    /// was edited after being applied is applied again under its new checksum
    /// instead of being reported as modified.
    ///
    /// The call only drops: it applies no migration, rewrites no script body, and
    /// re-creates no checkpoint marker. The objects it drops are exactly the
    /// objects [`clean_manifest`](Self::clean_manifest) reports, and the names are
    /// returned so the caller can report them to its operator.
    ///
    /// The default refuses the call with [`Error::CleanUnsupported`] before
    /// anything reaches the database.
    fn clean(&mut self) -> Result<Vec<String>, Error> {
        Err(Error::CleanUnsupported)
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
///
/// # Termination contract
///
/// A transaction opened by [`MigrationBackend::begin`] ends through one of the two
/// terminal calls: [`commit`](Self::commit) makes its work durable, and
/// [`rollback`](Self::rollback) discards it. The contract covers the failing paths
/// as much as the successful one:
///
/// * Dropping a transaction without a terminal call must leave no open database
///   transaction behind. An implementation that owns one ends it in its `Drop`, so
///   abandoning a transaction on an error path never keeps row and table locks held.
/// * A failed [`commit`](Self::commit) consumes the transaction all the same: the
///   work it tried to make durable is not applied, and no open database transaction
///   survives the call.
/// * A failed [`rollback`](Self::rollback) reports the driver error, and the
///   transaction is gone in either case.
///
/// The runner drives every transactional step through [`with_transaction`], which
/// commits on success and rolls back on failure, so the contract is enforced in one
/// place instead of at each call site. It has to hold: [`LockGuard`] releases the
/// exclusive migration lock as soon as the runner propagates an error, so a
/// transaction left open on that path would hold row and table locks while another
/// runner already owns the lock.
pub trait MigrationTransaction {
    /// Executes a raw SQL script within the transaction.
    fn execute(&mut self, sql: &str) -> Result<(), Error>;

    /// Reports the canonical digest of the catalog the transaction sees.
    ///
    /// A cold start reads it inside the very transaction that records the stage
    /// marker, once every stage script has been applied, so the marker anchors
    /// the digest of the schema the stage produced under the very engine that
    /// executes it, and the row becomes visible together with the schema it
    /// describes. The default reports `None`, which keeps the digest the caller
    /// supplied.
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
    ///
    /// A successful commit makes the work durable. A failing commit consumes the
    /// transaction all the same and must leave no open database transaction behind,
    /// as the termination contract of [`MigrationTransaction`] requires.
    fn commit(self) -> Result<(), Error>;

    /// Rolls back the active transaction.
    ///
    /// A failing rollback still terminates the transaction: the driver error is
    /// reported, and no open database transaction survives the call.
    fn rollback(self) -> Result<(), Error>;
}

/// Runs `work` inside one transaction and terminates that transaction either way.
///
/// The closure receives the open transaction of `guard`, applies the script, and
/// writes the history rows it needs. A successful closure commits; a failing one is
/// rolled back before the error is propagated, whichever step failed — the script,
/// a history write, or the commit itself. The rollback is best effort: its own
/// failure is logged and never replaces the error of `work`, which is the
/// diagnostic that names what went wrong.
///
/// Funnelling every transactional step through this helper is what enforces the
/// termination contract of [`MigrationTransaction`]: [`LockGuard`] releases the
/// exclusive migration lock as soon as the runner propagates the error, so no open
/// database transaction may outlive that lock.
pub(super) fn with_transaction<'g, B, T>(
    guard: &'g mut LockGuard<'_, B>,
    work: impl FnOnce(&mut B::Transaction<'g>) -> Result<T, Error>,
) -> Result<T, Error>
where
    B: MigrationBackend + 'g,
{
    let mut tx = guard.begin()?;

    let outcome = work(&mut tx);

    match outcome {
        Ok(value) => tx.commit().map(|()| value),
        Err(error) => {
            if let Err(rollback_error) = tx.rollback() {
                tracing::warn!(
                    error = %rollback_error,
                    "failed to roll back an abandoned migration transaction"
                );
            }
            Err(error)
        }
    }
}
