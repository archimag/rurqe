//! Migration protocol over a client `SqliteConnection`.
//!
//! The module carries the lifecycle onto the connection an application already
//! owns: the history rows of `__rurqe_schema_history`, the write lock that
//! serialises a run, and the transactional boundaries of the runner execute
//! through that connection, so a migration run reuses the session already open
//! instead of dialling a connection of its own.
//!
//! SQLite materialises its single-writer lock as a transaction, and a value that
//! opened one cannot outlive the call that opened it, so the run lock is an open
//! `BEGIN IMMEDIATE` the adapter tracks rather than a borrow it keeps. Two cases
//! are told apart:
//!
//! * A connection sitting in autocommit mode holds no transaction of its own, so
//!   the adapter takes the write lock with `BEGIN IMMEDIATE` — waiting for it
//!   through the driver busy handler up to the configured timeout — and commits it
//!   once the run ends. Every step of the run nests inside that lock as a
//!   savepoint, so a failing script discards its own statements while the steps
//!   that already succeeded stay in the transaction the run commits.
//! * A connection the caller has already placed inside a transaction serialises
//!   the writers itself, so the run nests inside that transaction: the adapter
//!   opens no lock and commits nothing, and the caller keeps the outcome of its
//!   own transaction. A script marked non-transactional is refused in that case,
//!   because the statements such a script exists for are exactly the ones SQLite
//!   rejects inside a transaction.
//!
//! The adapter is synchronous and blocking, exactly like the standalone backend:
//! invoke it from a blocking context, never directly on an asynchronous worker
//! thread.

use core::borrow::BorrowMut;
use std::time::Duration;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::warn_if_in_tokio_runtime;

use crate::introspect::introspect_sqlite;
use crate::SqliteConnection;

use super::history::{
    backend_error, delete_entry_sql, insert_entry_sql, read_history_rows, release_savepoint_sql,
    rollback_savepoint_sql, update_checksum_sql, SQLITE_STEP_SAVEPOINT,
};
use super::{left_open_message, DEFAULT_SQLITE_LOCK_TIMEOUT};

/// The write lock a run holds on the client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunLock {
    /// The run holds no lock of its own.
    None,
    /// The adapter opened the write lock and commits it when the run ends.
    Owned,
    /// A transaction the caller already held serialises the run.
    Shared,
}

/// [`MigrationBackend`] over the SQLite connection an application already owns.
///
/// The adapter borrows a [`SqliteConnection`] for the length of a run, so the
/// application keeps the handle — and the dialect bound to it — once the migration
/// finishes. The connection is named through `B`, which is the owned connection of
/// [`SqliteConnection::new`] as well as the borrowed handle of
/// [`SqliteConnection::borrowed`], so a pooled checkout drives the very protocol an
/// owned connection drives.
///
/// The catalog digest a step records comes from the same introspection the rest of
/// the toolchain uses, so a cold start anchors the digest `schema.lock` and
/// `checkpoint.toml` carry whatever engine executed the declarative stage.
pub struct SqliteConnectionMigrationBackend<'c, B: BorrowMut<::rusqlite::Connection>> {
    /// The connection the adapter borrows for the length of a run.
    connection: &'c mut SqliteConnection<B>,
    /// The write lock the run currently holds.
    run_lock: RunLock,
    /// How long a run waits for the write lock before giving up.
    lock_timeout: Duration,
    /// The diagnostic of the failure that left the connection unusable.
    ///
    /// A non-transactional script that ends with an open transaction is recovered
    /// from by rolling that transaction back. When the rollback itself fails, the
    /// connection stays inside the abandoned transaction and SQLite rejects every
    /// following `BEGIN` with a transaction-state diagnostic instead of the failure
    /// that caused it, so the adapter is poisoned with the composed cause and every
    /// later step reports it.
    poisoned: Option<String>,
}

impl<'c, B: BorrowMut<::rusqlite::Connection>> SqliteConnectionMigrationBackend<'c, B> {
    /// Borrows `connection` for a migration run.
    ///
    /// The session of the connection is the session the run drives, so nothing
    /// about it is opened or closed here: the caller keeps the handle it owns, and
    /// the adapter hands it back once the run finishes.
    pub fn new(connection: &'c mut SqliteConnection<B>) -> Self {
        Self {
            connection,
            run_lock: RunLock::None,
            lock_timeout: DEFAULT_SQLITE_LOCK_TIMEOUT,
            poisoned: None,
        }
    }

    /// Overrides how long a run waits for the write lock before giving up.
    ///
    /// SQLite serialises writers through a single lock on the database file. A run
    /// that cannot take it within the timeout is rejected with a diagnostic naming
    /// the wait, so a runner blocked by a stuck peer fails fast instead of hanging
    /// forever.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Whether a failed rollback left the connection unusable.
    ///
    /// A poisoned adapter rejects every later step and reports the composed cause of
    /// the run that poisoned it, so the caller knows the connection has to be
    /// replaced instead of retried.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// Introspects the schema catalog of the wrapped connection.
    ///
    /// The catalog is read through the very introspection the rest of the toolchain
    /// uses, so the history table stays out of it and the reported types follow the
    /// SQLite affinity rules.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(self.connection.connection()).map_err(backend_error)
    }

    /// Rejects a step on a connection a failed rollback left unusable.
    ///
    /// Nothing can be recovered from inside the adapter: the connection is still
    /// inside the abandoned transaction the script opened, so SQLite answers every
    /// `BEGIN IMMEDIATE` with its transaction-state diagnostic. Reporting the
    /// composed cause of the poisoning instead keeps the failure of the script — and
    /// of the rollback that could not undo it — in front of the operator.
    fn reject_poisoned(&self) -> Result<(), Error> {
        match &self.poisoned {
            Some(cause) => Err(Error::Backend(format!(
                "the SQLite migration connection is unusable after a failed rollback: {cause}"
            ))),
            None => Ok(()),
        }
    }
}

impl<B: BorrowMut<::rusqlite::Connection>> MigrationBackend
    for SqliteConnectionMigrationBackend<'_, B>
{
    type Transaction<'a>
        = SqliteConnectionMigrationTransaction<'a>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.connection
            .connection_mut()
            .execute_batch(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        read_history_rows(self.connection.connection())
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.reject_poisoned()?;

        let connection = self.connection.connection_mut();

        // A transaction is already open — the run lock of this adapter or one the
        // caller holds — so the step nests inside it as a savepoint instead of
        // opening a second transaction on the same session.
        if !connection.is_autocommit() {
            connection
                .execute_batch(&format!("SAVEPOINT {SQLITE_STEP_SAVEPOINT}"))
                .map_err(backend_error)?;
            return Ok(SqliteConnectionMigrationTransaction {
                step: Some(ClientStep::Savepoint(connection)),
            });
        }

        let transaction = connection
            .transaction_with_behavior(::rusqlite::TransactionBehavior::Immediate)
            .map_err(backend_error)?;
        Ok(SqliteConnectionMigrationTransaction {
            step: Some(ClientStep::Transaction(transaction)),
        })
    }

    /// Executes a script marked non-transactional in autocommit mode.
    ///
    /// A run holding the write lock as its own `BEGIN IMMEDIATE` commits that
    /// transaction first, because the statements a script marks itself
    /// non-transactional for are exactly the ones SQLite rejects inside a
    /// transaction, and takes the lock again afterwards so the history row of the
    /// script and the remaining steps of the run stay serialised. A script that
    /// fails midway therefore leaves its completed statements in the database: that
    /// is the documented contract of a non-transactional script, and the history row
    /// records the failure for a subsequent `repair`. A script that leaves a
    /// transaction open is rejected instead, and a rollback that fails leaves the
    /// connection inside the abandoned transaction, so the adapter is poisoned with
    /// the composed cause.
    ///
    /// A run nesting inside a transaction of the caller owns no lock to commit, so
    /// the script is refused rather than executed inside a transaction it exists to
    /// avoid.
    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        self.reject_poisoned()?;

        match self.run_lock {
            RunLock::Owned => {
                self.connection
                    .connection_mut()
                    .execute_batch("COMMIT")
                    .map_err(backend_error)?;
                self.run_lock = RunLock::None;
            }
            RunLock::Shared => {
                return Err(Error::Backend(String::from(
                    "a script marked non-transactional cannot run on a SQLite connection that is \
                     already inside a transaction of the caller; commit or roll back that \
                     transaction before running the migration",
                )));
            }
            RunLock::None => {}
        }

        let result = self
            .connection
            .connection_mut()
            .execute_batch(sql)
            .map_err(backend_error);

        if !self.connection.connection().is_autocommit() {
            // The script opened a transaction it never closed. The leftover
            // transaction is rolled back first, so the connection stays usable for
            // the failure record, and the diagnostic names the imbalance of the
            // script instead of the lock error a re-take would report.
            let rollback = self
                .connection
                .connection_mut()
                .execute_batch("ROLLBACK")
                .map_err(backend_error);
            let message = left_open_message(result, rollback);
            // A rollback that failed leaves the connection inside the abandoned
            // transaction, where SQLite rejects every following `BEGIN IMMEDIATE`
            // with a transaction-state diagnostic that hides the failure of the
            // script and even prevents the failed attempt from being recorded. The
            // adapter is poisoned instead, so every later step reports the composed
            // cause of this run.
            if !self.connection.connection().is_autocommit() {
                self.poisoned = Some(message.clone());
            }
            return Err(Error::Backend(message));
        }

        // Take the write lock again so the history row of the script and the
        // remaining steps of the run stay serialised.
        if let Err(error) = self.acquire_lock() {
            tracing::warn!(
                error = %error,
                "failed to re-take the SQLite migration lock after a non-transactional script"
            );
        }

        result
    }

    /// Takes the SQLite write lock for the duration of a migration run.
    ///
    /// SQLite materialises its single-writer lock as a transaction, so an
    /// autocommit connection takes it through `BEGIN IMMEDIATE` and holds it until
    /// [`release_lock`](MigrationBackend::release_lock) commits that transaction. A
    /// connection the caller has already placed inside a transaction already
    /// serialises the writers, so the run nests inside it and the caller keeps the
    /// outcome of its own transaction.
    fn acquire_lock(&mut self) -> Result<(), Error> {
        warn_if_in_tokio_runtime("SqliteConnectionMigrationBackend::acquire_lock");
        self.reject_poisoned()?;

        if self.run_lock != RunLock::None {
            return Ok(());
        }

        let timeout = self.lock_timeout;
        let held_by_caller = {
            let connection = self.connection.connection_mut();
            connection.busy_timeout(timeout).map_err(backend_error)?;
            !connection.is_autocommit()
        };

        if held_by_caller {
            self.run_lock = RunLock::Shared;
            return Ok(());
        }

        self.connection
            .connection_mut()
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|err| {
                Error::Backend(format!(
                    "could not acquire the SQLite write lock within {timeout:?}: {err}; another \
                     migration runner may be holding it, retry once that run finishes"
                ))
            })?;
        self.run_lock = RunLock::Owned;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        let lock = core::mem::replace(&mut self.run_lock, RunLock::None);

        // A run that holds nothing — or nests inside a transaction of the caller —
        // has nothing to commit: the caller owns the outcome of that transaction.
        if lock != RunLock::Owned {
            return Ok(());
        }

        let connection = self.connection.connection_mut();
        // A non-transactional script ends the lock transaction and a failed re-take
        // leaves the connection in autocommit, so a connection holding no
        // transaction has nothing to commit — the driver would reject the statement
        // with `cannot commit - no transaction is active`.
        if connection.is_autocommit() {
            return Ok(());
        }
        connection.execute_batch("COMMIT").map_err(backend_error)
    }
}

impl<B: BorrowMut<::rusqlite::Connection>> Drop for SqliteConnectionMigrationBackend<'_, B> {
    fn drop(&mut self) {
        // The connection outlives the adapter, so a lock the run still holds is
        // committed here instead of being left open on the connection the caller
        // keeps.
        if self.run_lock != RunLock::Owned {
            return;
        }
        if let Err(err) = self.release_lock() {
            tracing::warn!(error = %err, "failed to release the SQLite migration lock");
        }
    }
}

/// One step of a migration run over a client connection.
enum ClientStep<'a> {
    /// A step owning the transaction it opened itself.
    Transaction(::rusqlite::Transaction<'a>),
    /// A step nested in an open transaction as a savepoint.
    Savepoint(&'a mut ::rusqlite::Connection),
}

/// Transactional boundary of a [`SqliteConnectionMigrationBackend`] step.
///
/// A step started outside a transaction opens one, while a step started inside a
/// transaction the connection already holds — the run lock of the adapter or a
/// transaction of the caller — nests inside a savepoint, so a migration never
/// discards the work of its host. Dropping the boundary without a terminal call
/// rolls the block back, so an abandoned step never leaves an open transaction
/// behind.
pub struct SqliteConnectionMigrationTransaction<'a> {
    step: Option<ClientStep<'a>>,
}

impl<'a> SqliteConnectionMigrationTransaction<'a> {
    fn current(&mut self) -> Result<&mut ClientStep<'a>, Error> {
        self.step.as_mut().ok_or_else(|| {
            Error::Backend(String::from("sqlite migration transaction already finished"))
        })
    }

    fn connection(&mut self) -> Result<&::rusqlite::Connection, Error> {
        match self.current()? {
            ClientStep::Transaction(transaction) => Ok(&**transaction),
            ClientStep::Savepoint(connection) => Ok(&**connection),
        }
    }

    /// Introspects the schema catalog the step sees.
    ///
    /// The catalog is read over the connection of the step, so a migration observes
    /// the changes it has already applied without committing them.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(self.connection()?).map_err(backend_error)
    }

    fn run(&mut self, sql: &str, params: impl ::rusqlite::Params) -> Result<(), Error> {
        self.connection()?
            .execute(sql, params)
            .map(|_| ())
            .map_err(backend_error)
    }
}

impl MigrationTransaction for SqliteConnectionMigrationTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.connection()?.execute_batch(sql).map_err(backend_error)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        let sql = insert_entry_sql();
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());

        self.run(
            &sql,
            ::rusqlite::params![
                entry.installed_rank,
                version,
                entry.description,
                entry.kind.as_str(),
                entry.script,
                checksum,
                entry.installed_on,
                entry.execution_time_ms,
                entry.success,
            ],
        )
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        let sql = delete_entry_sql();
        self.run(&sql, ::rusqlite::params![installed_rank])
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql();
        self.run(&sql, ::rusqlite::params![checksum.to_hex(), script])
    }

    fn commit(mut self) -> Result<(), Error> {
        let step = self.step.take().ok_or_else(|| {
            Error::Backend(String::from("sqlite migration transaction already finished"))
        })?;

        match step {
            ClientStep::Transaction(transaction) => transaction.commit().map_err(backend_error),
            ClientStep::Savepoint(connection) => connection
                .execute_batch(&release_savepoint_sql())
                .map_err(backend_error),
        }
    }

    fn rollback(mut self) -> Result<(), Error> {
        let step = self.step.take().ok_or_else(|| {
            Error::Backend(String::from("sqlite migration transaction already finished"))
        })?;

        match step {
            ClientStep::Transaction(transaction) => transaction.rollback().map_err(backend_error),
            ClientStep::Savepoint(connection) => connection
                .execute_batch(&rollback_savepoint_sql())
                .map_err(backend_error),
        }
    }
}

impl Drop for SqliteConnectionMigrationTransaction<'_> {
    fn drop(&mut self) {
        let Some(step) = self.step.take() else {
            return;
        };

        let outcome = match step {
            ClientStep::Transaction(transaction) => transaction.rollback(),
            ClientStep::Savepoint(connection) => connection.execute_batch(&rollback_savepoint_sql()),
        };

        if let Err(err) = outcome {
            tracing::warn!(error = %err, "failed to rollback sqlite migration transaction");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> SqliteConnection {
        SqliteConnection::new(
            ::rusqlite::Connection::open_in_memory().expect("the in-memory database opens"),
        )
    }

    #[test]
    fn a_connection_a_failed_rollback_left_unusable_reports_the_cause_instead_of_stepping() {
        let mut connection = connection();
        let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
        backend.poisoned = Some(
            "disk I/O error; the script also left a transaction open and rolling it back failed"
                .to_owned(),
        );

        let error = match backend.begin() {
            Ok(_) => panic!("a poisoned backend opens no step"),
            Err(error) => error,
        };
        let message = match &error {
            Error::Backend(message) => message.clone(),
            other => panic!("unexpected error: {other:?}"),
        };
        assert!(message.contains("unusable"), "{message}");
        assert!(message.contains("disk I/O error"), "{message}");

        let error = backend
            .execute_non_transactional("CREATE TABLE users (id INTEGER NOT NULL);")
            .expect_err("a poisoned backend executes no script");
        let message = match &error {
            Error::Backend(message) => message.clone(),
            other => panic!("unexpected error: {other:?}"),
        };
        assert!(message.contains("unusable"), "{message}");
        assert!(message.contains("rolling it back failed"), "{message}");
    }

    #[test]
    fn a_failed_non_transactional_script_keeps_its_own_error() {
        let mut connection = connection();

        let error = {
            let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
            backend.acquire_lock().expect("the write lock is taken");

            let error = backend
                .execute_non_transactional("BEGIN; CREATE TABLE oops (;")
                .expect_err("the malformed statement fails the script");

            assert!(
                !backend.is_poisoned(),
                "a successful rollback poisons nothing"
            );
            error
        };

        let message = match &error {
            Error::Backend(message) => message.clone(),
            other => panic!("unexpected error: {other:?}"),
        };
        assert!(
            message.contains("syntax error"),
            "the driver's own failure must reach the operator: {message}"
        );
        assert!(
            message.contains("left a transaction open"),
            "the leftover transaction is still reported as context: {message}"
        );

        // The rollback runs before the error is reported, so the connection stays
        // usable for the failure record.
        connection
            .execute_batch("CREATE TABLE users (id INTEGER NOT NULL);")
            .expect("the connection remains usable");
    }
}
