//! Migration protocol over a client [`PgConnection`].
//!
//! The module carries the lifecycle onto the connection an application already
//! owns: the history rows of `__rurqe_schema_history`, the session-scoped advisory
//! lock, and the transactional boundaries of the runner execute through that
//! connection, so a migration run reuses the session already open instead of
//! dialling a connection of its own.
//!
//! The transactional boundary is the bookkeeping of the connection rather than a
//! driver transaction created beside it. A step started outside a transaction
//! opens one, while a step started inside a transaction the caller already holds
//! nests inside a savepoint that the guard of the connection creates, rolls back,
//! and releases, so a migration never discards the work of its host. A step
//! abandoned without a terminal call is rolled back when its guard is dropped, and
//! a commit the driver refuses rolls the transaction back as well, so the runner
//! never hands a connection carrying an abandoned transaction to the next step.
//!
//! A step never opens on a session the adapter cannot vouch for either. The
//! application shares this session, so a transaction it left open through the raw
//! handle is invisible to the bookkeeping of the connection and would simply
//! enclose the step: the backend returns such a session to an idle state before
//! it opens a block and before it runs an autocommit script, so the leaked work
//! is discarded while the step's own work proceeds.
//!
//! The advisory lock is session-scoped, exactly as it is for the standalone
//! backend: it belongs to the connection the caller holds, so it is taken once per
//! run and released when the run finishes, and a release the server never
//! confirmed is reported instead of being assumed.

use std::borrow::BorrowMut;
use std::time::Duration;

use rurqe_core::query::{TransactionBackend, TransactionGuard};
use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use rurqe_migrate::introspect::SchemaSelection;
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::{warn_if_in_tokio_runtime, ThreadSleeper};

use crate::backend::sync_conn::PgConnection;
use crate::introspect::{introspect_postgres_selection, DEFAULT_SCHEMA};

use super::history::{
    delete_entry_sql, insert_entry_sql, update_checksum_sql, HISTORY_COLUMN_COUNT,
};
use super::{
    apply_unlock_answer, backend_error, backend_failure, poll_advisory_lock, read_history_rows,
    ADVISORY_LOCK_RETRY_INTERVAL, DEFAULT_ADVISORY_LOCK_TIMEOUT, MIGRATION_LOCK_ID,
};

/// [`MigrationBackend`] over the connection an application already owns.
///
/// The adapter borrows [`PgConnection`] for the length of a run, so the
/// application keeps the handle — and the dialect bound to it — once the migration
/// finishes. The connection is named through `B`, which is the owned client of
/// [`PgConnection::new`] as well as the borrowed handle of
/// [`PgConnection::borrowed`], so a pooled checkout drives the very protocol an
/// owned connection drives.
///
/// The catalog digest a step records follows the schema selection the adapter
/// carries, exactly as the selection of the standalone backend does:
/// [`with_schema`](Self::with_schema) names the primary schema and
/// [`with_schemas`](Self::with_schemas) adds the secondary ones, so the digest
/// describes the schemas the project declares instead of an unrelated `public`
/// catalog.
///
/// The session a step runs on is the session the application shares, so the
/// adapter proves it idle before it opens a block or runs an autocommit script: a
/// transaction the application left open through the raw handle is rolled back
/// instead of enclosing the step, while a transaction a guard of the connection
/// holds stays untouched and makes the step nest inside a savepoint of it.
pub struct PgConnectionMigrationBackend<'c, B: BorrowMut<postgres::Client>> {
    connection: &'c mut PgConnection<B>,
    selection: SchemaSelection,
    lock_held: bool,
    lock_timeout: Duration,
}

impl<'c, B: BorrowMut<postgres::Client>> PgConnectionMigrationBackend<'c, B> {
    /// Borrows `connection` for a migration run.
    ///
    /// The session of the connection is the session the run drives, so nothing
    /// about it is opened or closed here: the caller keeps the handle it owns,
    /// and the adapter hands it back once the run finishes.
    pub fn new(connection: &'c mut PgConnection<B>) -> Self {
        Self {
            connection,
            selection: SchemaSelection::single(DEFAULT_SCHEMA),
            lock_held: false,
            lock_timeout: DEFAULT_ADVISORY_LOCK_TIMEOUT,
        }
    }

    /// Selects the PostgreSQL schemas the recorded catalog digest describes.
    ///
    /// `public` suits a deployment keeping its tables in the default schema, while
    /// a project owning schemas of its own names them here, so the digest a step
    /// records describes the very tables the declarative stage declares. The
    /// selector mirrors the one of the standalone backend, so both adapters record
    /// the same catalog for the same project.
    pub fn with_schemas(mut self, selection: SchemaSelection) -> Self {
        self.selection = selection;
        self
    }

    /// Selects the one PostgreSQL schema the recorded catalog digest describes.
    ///
    /// The single name becomes a selection holding no secondary schema, so a
    /// project describing one schema records the same catalog through either
    /// selector.
    pub fn with_schema(self, schema: impl Into<String>) -> Self {
        self.with_schemas(SchemaSelection::single(schema))
    }

    /// The primary PostgreSQL schema the recorded catalog digest describes.
    pub fn schema(&self) -> &str {
        self.selection.primary()
    }

    /// Overrides how long a run waits for the advisory lock before giving up.
    ///
    /// A migration that cannot take the lock within the timeout is rejected with a
    /// diagnostic naming the lock, so a run blocked by a stuck peer fails fast
    /// instead of hanging forever.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Introspects the catalog of the selected schemas.
    ///
    /// The selection is bound as one array parameter, so the returned catalog
    /// describes the primary schema and every secondary schema the project
    /// declares, and the recorded digest covers all of them.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        let selection = self.selection.clone();
        introspect_postgres_selection(self.connection.client_mut(), &selection)
            .map_err(backend_failure)
    }
}

impl<B: BorrowMut<postgres::Client>> MigrationBackend for PgConnectionMigrationBackend<'_, B> {
    type Transaction<'a>
        = PgConnectionMigrationTransaction<'a, B>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.connection
            .client_mut()
            .batch_execute(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        read_history_rows(self.connection.client_mut())
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let selection = self.selection.clone();
        // A step never joins a transaction the application left open through the
        // raw handle: the repair rolls such a session back first, while a session
        // the bookkeeping records as open belongs to a block of this connection —
        // the caller's own transaction or a nested savepoint — and stays intact.
        self.connection
            .repair_idle_session()
            .map_err(backend_failure)?;
        // The guard of the connection decides whether this step opens a
        // transaction or nests inside a savepoint of the one the caller holds, so
        // the adapter never issues its own control statement.
        let guard = TransactionGuard::begin(&mut *self.connection).map_err(backend_failure)?;

        Ok(PgConnectionMigrationTransaction {
            guard: Some(guard),
            selection,
        })
    }

    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        // A script runs in autocommit, so the session is proven idle first: a
        // transaction the application left open through the raw handle is rolled
        // back instead of enclosing the script. A caller sitting inside a
        // transaction of its own keeps it there, because the bookkeeping records
        // the block and the repair leaves it untouched.
        self.connection
            .repair_idle_session()
            .map_err(backend_failure)?;
        self.connection
            .client_mut()
            .batch_execute(sql)
            .map_err(backend_error)
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        warn_if_in_tokio_runtime("PgConnectionMigrationBackend::acquire_lock");

        let lock_id = MIGRATION_LOCK_ID;
        let timeout = self.lock_timeout;

        tracing::debug!(
            lock_id,
            timeout = ?timeout,
            "acquiring the PostgreSQL advisory lock through the client connection"
        );

        let outcome = {
            let client = self.connection.client_mut();
            poll_advisory_lock(
                lock_id,
                timeout,
                ADVISORY_LOCK_RETRY_INTERVAL,
                &ThreadSleeper,
                || {
                    let row = client
                        .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_id])
                        .map_err(backend_error)?;
                    row.try_get(0).map_err(backend_error)
                },
            )
        };

        match outcome {
            Ok(attempts) => {
                self.lock_held = true;
                tracing::debug!(
                    lock_id,
                    attempts = attempts as u64,
                    "acquired the PostgreSQL advisory lock"
                );
                Ok(())
            }
            Err(err) => {
                tracing::warn!(
                    lock_id,
                    error = %err,
                    "could not acquire the PostgreSQL advisory lock"
                );
                Err(err)
            }
        }
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        // A run that took no lock — or already released it — has nothing to
        // unlock, and asking the server anyway would answer `false` for a run that
        // never held the lock.
        if !self.lock_held {
            return Ok(());
        }

        tracing::debug!(
            lock_id = MIGRATION_LOCK_ID,
            "releasing the PostgreSQL advisory lock"
        );

        // The answer of the statement and the diagnostic of a statement that never
        // produced one are kept apart exactly as they are for the standalone
        // backend: a confirmed answer proves the session holds no lock any more,
        // while a driver failure is reported instead of being assumed away.
        let answer = self
            .connection
            .client_mut()
            .query_one("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID])
            .map_err(backend_error)
            .and_then(|row| row.try_get(0).map_err(backend_error));

        apply_unlock_answer(MIGRATION_LOCK_ID, &mut self.lock_held, answer)
    }
}

impl<B: BorrowMut<postgres::Client>> Drop for PgConnectionMigrationBackend<'_, B> {
    fn drop(&mut self) {
        // The session outlives the adapter, so a lock the run still holds is
        // released here instead of being left to the connection the caller keeps.
        if !self.lock_held {
            return;
        }
        if let Err(err) = self.release_lock() {
            tracing::warn!(error = %err, "failed to release the migration advisory lock");
        }
    }
}

/// Transactional boundary of a [`PgConnectionMigrationBackend`] step.
///
/// The boundary is the transaction of the connection itself, so a step started
/// inside a transaction of the caller nests inside a savepoint instead of opening a
/// second transaction on the same session. Dropping the boundary without a terminal
/// call rolls the block back — the enclosing transaction for an outermost step, its
/// own savepoint for a nested one — so an abandoned step never leaves an open
/// transaction behind.
pub struct PgConnectionMigrationTransaction<'c, B: BorrowMut<postgres::Client>> {
    guard: Option<TransactionGuard<'c, PgConnection<B>>>,
    selection: SchemaSelection,
}

impl<'c, B: BorrowMut<postgres::Client>> PgConnectionMigrationTransaction<'c, B> {
    /// The guard of the open block, or the failure of a repeated terminal call.
    fn guard_mut(&mut self) -> Result<&mut TransactionGuard<'c, PgConnection<B>>, Error> {
        self.guard.as_mut().ok_or_else(|| {
            Error::Backend(String::from("postgres migration transaction already finished"))
        })
    }

    /// The client handle of the open block.
    fn client_mut(&mut self) -> Result<&mut postgres::Client, Error> {
        Ok(self.guard_mut()?.client_mut())
    }

    /// Takes the guard out of a block that reached a terminal call.
    fn take_guard(&mut self) -> Result<TransactionGuard<'c, PgConnection<B>>, Error> {
        self.guard.take().ok_or_else(|| {
            Error::Backend(String::from("postgres migration transaction already finished"))
        })
    }

    /// Introspects the selected schemas as the open transaction sees them.
    ///
    /// The catalog statements run over the connection itself, so a step reads the
    /// changes it has applied so far and nobody else observes them until the
    /// enclosing transaction commits.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        let selection = self.selection.clone();
        introspect_postgres_selection(self.guard_mut()?.client_mut(), &selection)
            .map_err(backend_failure)
    }
}

impl<B: BorrowMut<postgres::Client>> MigrationTransaction
    for PgConnectionMigrationTransaction<'_, B>
{
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        // A script is a sequence of statements, so it goes through the simple
        // protocol of the driver rather than a single prepared statement.
        self.client_mut()?.batch_execute(sql).map_err(backend_error)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        let sql = insert_entry_sql();
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());
        let kind = entry.kind.as_str();

        let params: [&(dyn postgres::types::ToSql + Sync); HISTORY_COLUMN_COUNT] = [
            &entry.installed_rank,
            &version,
            &entry.description,
            &kind,
            &entry.script,
            &checksum,
            &entry.installed_on,
            &entry.execution_time_ms,
            &entry.success,
        ];

        self.client_mut()?
            .execute(&sql, &params)
            .map(|_| ())
            .map_err(backend_error)
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        let sql = delete_entry_sql();
        self.client_mut()?
            .execute(&sql, &[&installed_rank])
            .map(|_| ())
            .map_err(backend_error)
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql();
        let hex = checksum.to_hex();
        self.client_mut()?
            .execute(&sql, &[&hex, &script])
            .map(|_| ())
            .map_err(backend_error)
    }

    fn commit(mut self) -> Result<(), Error> {
        // A commit the driver refuses consumes the transaction all the same: the
        // guard rolls the block back on the way out, so no open transaction
        // survives the call.
        let guard = self.take_guard()?;
        guard.commit().map_err(backend_failure)
    }

    fn rollback(mut self) -> Result<(), Error> {
        let Some(mut guard) = self.guard.take() else {
            return Ok(());
        };

        // The rollback is issued here instead of being left to the guard's `Drop`,
        // so the answer of the driver reaches the caller. The guard is dropped
        // right after and closes the block it recorded; repeating the statement is
        // harmless on PostgreSQL, which accepts `ROLLBACK` in an idle session with
        // a warning and keeps a savepoint after `ROLLBACK TO SAVEPOINT`.
        let answer = if guard.is_outer() {
            guard.rollback()
        } else {
            match guard.savepoint().cloned() {
                Some(savepoint) => guard.rollback_to_savepoint(&savepoint),
                None => Ok(()),
            }
        };

        drop(guard);

        answer.map_err(backend_failure)
    }
}
