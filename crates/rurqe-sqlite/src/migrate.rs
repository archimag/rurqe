//! Migration protocol over a `rusqlite` connection.
//!
//! The module carries the engine-facing half of the SQLite lifecycle: the
//! statements recording `__rurqe_schema_history`, the write lock that serialises
//! the runs, and the [`MigrationBackend`] adapters the runner drives. Every
//! transactional step runs inside a driver transaction, so a migration script and
//! its history row are always applied or discarded together.
//!
//! The adapter is synchronous and blocking: every statement runs on the calling
//! thread, and [`acquire_lock`](MigrationBackend::acquire_lock) waits for the
//! SQLite write lock through the driver busy handler. Invoke it from a blocking
//! context — `tokio::task::spawn_blocking`, a dedicated thread, or plain
//! synchronous code — never directly on an asynchronous worker thread.
//!
//! The write lock is the database file itself, and SQLite materialises it as a
//! transaction: a run holds one open `BEGIN IMMEDIATE`, and every transactional
//! step nests in a savepoint of it. A script marked non-transactional commits the
//! lock first, because the statements such a script exists for are exactly the ones
//! SQLite rejects inside a transaction; the lock is taken again afterwards, so the
//! history row of the script and the remaining steps of the run stay serialised.
//!
//! `SqliteConnectionMigrationBackend` is the client half: it drives a
//! `SqliteConnection` the application already owns — owned or borrowed — so a
//! service migrates through the very connection it queries with.

use std::time::Duration;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::warn_if_in_tokio_runtime;

use crate::introspect::introspect_sqlite;
mod client;
mod history;

pub use history::{
    delete_entry_sql, insert_entry_sql, select_history_sql, update_checksum_sql, RawHistoryRow,
    HISTORY_COLUMN_COUNT,
};

pub use client::{SqliteConnectionMigrationBackend, SqliteConnectionMigrationTransaction};

use history::{
    backend_error, read_history_rows, release_savepoint_sql, rollback_savepoint_sql,
    SQLITE_STEP_SAVEPOINT,
};

/// Time a migration run waits for the SQLite write lock.
const DEFAULT_SQLITE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// [`MigrationBackend`] over a `rusqlite` connection.
///
/// The adapter owns the connection it drives: the run lock is a transaction of
/// that very connection, so a step nested in the lock is a savepoint of the same
/// session instead of a second one.
pub struct SqliteMigrationBackend {
    /// The connection the adapter owns, shared with the step it opens.
    conn: ::rusqlite::Connection,
    lock_held: bool,
    lock_timeout: Duration,
    /// The diagnostic of the failure that left the connection unusable.
    ///
    /// A non-transactional script that ends with an open transaction is recovered
    /// from by rolling that transaction back. When the rollback itself fails, the
    /// connection stays inside the abandoned transaction and SQLite rejects every
    /// following `BEGIN` with a transaction-state diagnostic instead of the failure
    /// that caused it, so the backend is poisoned with the composed cause and every
    /// later step reports it.
    poisoned: Option<String>,
}

impl SqliteMigrationBackend {
    /// Wraps an open SQLite connection.
    pub fn new(conn: ::rusqlite::Connection) -> Self {
        Self {
            conn,
            lock_held: false,
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

    /// The connection the adapter drives.
    ///
    /// The handle is read-only, because the adapter owns the transactional state of
    /// the session: a caller observes the database — the catalog, a fixture row —
    /// without taking the run over, while the migration protocol goes through the
    /// backend methods alone.
    pub fn connection(&self) -> &::rusqlite::Connection {
        &self.conn
    }

    /// Whether a failed rollback left the connection unusable.
    ///
    /// A poisoned backend rejects every later step and reports the composed cause of
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
    pub fn introspect_catalog(&self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(&self.conn).map_err(backend_error)
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

impl MigrationBackend for SqliteMigrationBackend {
    type Transaction<'a> = SqliteMigrationTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.conn
            .execute_batch(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        read_history_rows(&self.conn)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.reject_poisoned()?;
        if self.lock_held {
            self.conn
                .execute_batch(&format!("SAVEPOINT {SQLITE_STEP_SAVEPOINT}"))
                .map_err(backend_error)?;
            return Ok(SqliteMigrationTransaction {
                step: Some(SqliteStep::Savepoint(&mut self.conn)),
            });
        }

        let tx = self
            .conn
            .transaction_with_behavior(::rusqlite::TransactionBehavior::Immediate)
            .map_err(backend_error)?;
        Ok(SqliteMigrationTransaction {
            step: Some(SqliteStep::Transaction(tx)),
        })
    }

    /// Executes a script marked non-transactional in autocommit mode.
    ///
    /// The run lock is held as an open `BEGIN IMMEDIATE` transaction, and the
    /// statements a script marks itself non-transactional for are exactly the ones
    /// SQLite rejects inside a transaction — `VACUUM`, several `PRAGMA` settings. The
    /// lock transaction is committed first, so the script runs outside any
    /// transaction the runner opened and every statement it completes is durable on
    /// its own. A script that fails midway therefore leaves its completed statements
    /// in the database: that is the documented contract of a non-transactional
    /// script, and the history row records the failure for a subsequent `repair`. A
    /// script that leaves a transaction open is rejected instead: the leftover
    /// transaction is rolled back and the error the script itself raised, when there
    /// is one, is reported with the imbalance appended as context. A rollback that
    /// fails leaves the connection inside the abandoned transaction, so the backend
    /// is poisoned with the composed cause and every later step reports it instead of
    /// SQLite's transaction-state diagnostic.
    ///
    /// Between the `COMMIT` and the re-take below the write lock is free: SQLite
    /// materialises its single-writer lock only as a transaction, so another runner
    /// may hold the lock for as long as its own run lasts. A runner that cannot
    /// re-take the lock keeps going without it — every following step opens its own
    /// `IMMEDIATE` transaction — and
    /// [`release_lock`](MigrationBackend::release_lock) tolerates the missing lock
    /// transaction.
    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        self.reject_poisoned()?;
        if self.lock_held {
            self.conn.execute_batch("COMMIT").map_err(backend_error)?;
            self.lock_held = false;
        }

        let result = self.conn.execute_batch(sql).map_err(backend_error);

        if !self.conn.is_autocommit() {
            // The script opened a transaction it never closed. The leftover
            // transaction is rolled back first, so the connection stays usable for
            // the failure record, and the diagnostic names the imbalance of the
            // script instead of the lock error a re-take would report.
            let rollback = self.conn.execute_batch("ROLLBACK").map_err(backend_error);
            let message = left_open_message(result, rollback);
            // A rollback that failed leaves the connection inside the abandoned
            // transaction, where SQLite rejects every following `BEGIN IMMEDIATE`
            // with a transaction-state diagnostic that hides the failure of the
            // script and even prevents the failed attempt from being recorded. The
            // backend is poisoned instead, so every later step reports the composed
            // cause of this run.
            if !self.conn.is_autocommit() {
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
    /// SQLite materialises its single-writer lock as a transaction, so the lock is
    /// held as an open `BEGIN IMMEDIATE` until
    /// [`release_lock`](MigrationBackend::release_lock) commits it. A script marked
    /// non-transactional cannot run inside that transaction, so
    /// [`execute_non_transactional`](MigrationBackend::execute_non_transactional)
    /// commits the lock transaction before such a script and takes the lock again
    /// afterwards; between those statements another runner may hold the write lock
    /// for as long as its own run lasts.
    fn acquire_lock(&mut self) -> Result<(), Error> {
        warn_if_in_tokio_runtime("SqliteMigrationBackend::acquire_lock");
        let timeout = self.lock_timeout;
        self.conn.busy_timeout(timeout).map_err(backend_error)?;
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(|err| {
            Error::Backend(format!(
                "could not acquire the SQLite write lock within {timeout:?}: {err}; another \
                 migration runner may be holding it, retry once that run finishes"
            ))
        })?;
        self.lock_held = true;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        // The flag is cleared before the statement runs: a connection that is
        // already unusable cannot commit, and the file lock is released by the
        // driver once the connection closes.
        self.lock_held = false;
        // A non-transactional script ends the lock transaction and a failed re-take
        // leaves the connection in autocommit, so a connection holding no
        // transaction has nothing to commit — the driver would reject the statement
        // with `cannot commit - no transaction is active`.
        if self.conn.is_autocommit() {
            return Ok(());
        }
        self.conn.execute_batch("COMMIT").map_err(backend_error)
    }
}

impl Drop for SqliteMigrationBackend {
    fn drop(&mut self) {
        if !self.lock_held {
            return;
        }
        if let Err(err) = self.release_lock() {
            tracing::warn!(error = %err, "failed to release the SQLite migration lock");
        }
    }
}

/// The diagnostic of a non-transactional script that ended with an open transaction.
///
/// A script that failed keeps its own error: the driver's syntax or constraint
/// diagnostic is the cause an operator acts on, and the leftover transaction is only
/// context, so the imbalance is appended instead of replacing it. The bare imbalance
/// message is reserved for a script that completed successfully yet never closed the
/// transaction it opened — there the imbalance is the only thing wrong with the
/// script. A rollback that failed is reported the same way, after the imbalance it
/// could not undo, because the connection then stays inside the abandoned transaction
/// and the run cannot record its outcome.
fn left_open_message(script: Result<(), Error>, rollback: Result<(), Error>) -> String {
    // The remedy appended to every diagnostic.
    const REMEDY: &str = "balance the BEGIN statements of a script marked non-transactional";

    match (script, rollback) {
        // The script completed and its work was undone: the imbalance is the only
        // thing wrong with it.
        (Ok(()), Ok(())) => format!("non-transactional script left a transaction open; {REMEDY}"),
        (Err(error), Ok(())) => format!(
            "{}; the script also left a transaction open and its applied statements were \
             rolled back; {REMEDY}",
            error_message(error)
        ),
        // The rollback failed, so the connection stays inside the abandoned
        // transaction and every later statement is rejected: the rollback's own
        // failure is reported after the imbalance it could not undo.
        (Ok(()), Err(error)) => format!(
            "non-transactional script left a transaction open and rolling it back failed: {}; \
             the connection remains inside the abandoned transaction, so the run cannot record \
             its outcome and has to be restarted on a fresh connection; {REMEDY}",
            error_message(error)
        ),
        (Err(error), Err(rollback)) => format!(
            "{}; the script also left a transaction open and rolling it back failed: {}; the \
             connection remains inside the abandoned transaction, so the run cannot record its \
             outcome and has to be restarted on a fresh connection; {REMEDY}",
            error_message(error),
            error_message(rollback)
        ),
    }
}

/// The operator-facing text of a backend failure.
fn error_message(error: Error) -> String {
    match error {
        Error::Backend(message) => message,
        other => other.to_string(),
    }
}

/// One step of a SQLite migration run.
enum SqliteStep<'a> {
    /// A step that owns its native transaction.
    Transaction(::rusqlite::Transaction<'a>),
    /// A step nested in the transaction that holds the run lock.
    Savepoint(&'a mut ::rusqlite::Connection),
}

/// Transactional boundary of a [`SqliteMigrationBackend`] step.
pub struct SqliteMigrationTransaction<'a> {
    step: Option<SqliteStep<'a>>,
}

impl<'a> SqliteMigrationTransaction<'a> {
    fn current(&mut self) -> Result<&mut SqliteStep<'a>, Error> {
        self.step.as_mut().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })
    }

    fn connection(&mut self) -> Result<&::rusqlite::Connection, Error> {
        match self.current()? {
            SqliteStep::Transaction(tx) => Ok(&**tx),
            SqliteStep::Savepoint(conn) => Ok(&**conn),
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

impl MigrationTransaction for SqliteMigrationTransaction<'_> {
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
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })?;

        match step {
            SqliteStep::Transaction(tx) => tx.commit().map_err(backend_error),
            SqliteStep::Savepoint(conn) => conn
                .execute_batch(&release_savepoint_sql())
                .map_err(backend_error),
        }
    }

    fn rollback(mut self) -> Result<(), Error> {
        let step = self.step.take().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })?;

        match step {
            SqliteStep::Transaction(tx) => tx.rollback().map_err(backend_error),
            SqliteStep::Savepoint(conn) => conn
                .execute_batch(&rollback_savepoint_sql())
                .map_err(backend_error),
        }
    }
}

impl Drop for SqliteMigrationTransaction<'_> {
    fn drop(&mut self) {
        let Some(step) = self.step.take() else {
            return;
        };

        let outcome = match step {
            SqliteStep::Transaction(tx) => tx.rollback(),
            SqliteStep::Savepoint(conn) => conn.execute_batch(&rollback_savepoint_sql()),
        };

        if let Err(err) = outcome {
            tracing::warn!(error = %err, "failed to rollback sqlite migration transaction");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> SqliteMigrationBackend {
        SqliteMigrationBackend::new(
            ::rusqlite::Connection::open_in_memory().expect("the in-memory database opens"),
        )
    }

    #[test]
    fn the_composed_diagnostic_reports_the_imbalance_alone() {
        let message = left_open_message(Ok(()), Ok(()));

        assert!(
            message.starts_with("non-transactional script left a transaction open"),
            "{message}"
        );
        assert!(
            message.contains("balance the BEGIN statements"),
            "{message}"
        );
        assert!(!message.contains("failed"), "{message}");
    }

    #[test]
    fn the_composed_diagnostic_keeps_the_script_cause_over_a_successful_rollback() {
        let message = left_open_message(
            Err(Error::Backend("near \"oops\": syntax error".to_owned())),
            Ok(()),
        );

        assert!(
            message.starts_with("near \"oops\": syntax error"),
            "{message}"
        );
        assert!(
            message.contains("its applied statements were rolled back"),
            "{message}"
        );
        assert!(!message.contains("rolling it back failed"), "{message}");
    }

    #[test]
    fn the_composed_diagnostic_reports_a_rollback_that_could_not_undo_the_script() {
        let message = left_open_message(Ok(()), Err(Error::Backend("disk I/O error".to_owned())));

        assert!(
            message.starts_with("non-transactional script left a transaction open"),
            "{message}"
        );
        assert!(
            message.contains("rolling it back failed: disk I/O error"),
            "{message}"
        );
        assert!(message.contains("fresh connection"), "{message}");
    }

    #[test]
    fn the_composed_diagnostic_carries_both_the_script_and_the_rollback_failure() {
        let message = left_open_message(
            Err(Error::Backend("near \"oops\": syntax error".to_owned())),
            Err(Error::Backend("disk I/O error".to_owned())),
        );

        assert!(
            message.starts_with("near \"oops\": syntax error"),
            "{message}"
        );
        assert!(
            message.contains("rolling it back failed: disk I/O error"),
            "{message}"
        );
        assert!(
            message.contains("balance the BEGIN statements"),
            "{message}"
        );
    }

    #[test]
    fn a_backend_a_failed_rollback_left_unusable_reports_the_cause_instead_of_stepping() {
        let mut backend = backend();
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
}