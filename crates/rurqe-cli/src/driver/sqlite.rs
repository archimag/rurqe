//! [`MigrationBackend`] over a `rusqlite` connection.

use std::time::Duration;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::warn_if_in_tokio_runtime;

use crate::introspect::introspect_sqlite;

use super::history_sql::{
    backend_error, delete_entry_sql, insert_entry_sql, release_savepoint_sql,
    rollback_savepoint_sql, select_history_sql, update_checksum_sql, RawHistoryRow,
    SQLITE_STEP_SAVEPOINT,
};

/// Time a migration run waits for the SQLite write lock.
const DEFAULT_SQLITE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// [`MigrationBackend`] over a `rusqlite` connection.
///
/// The adapter is synchronous and blocking: every statement runs on the calling
/// thread, and [`acquire_lock`](MigrationBackend::acquire_lock) waits for the
/// SQLite write lock through the driver busy handler. Invoke it from a blocking
/// context — `tokio::task::spawn_blocking`, a dedicated thread, or plain
/// synchronous code — never directly on an asynchronous worker thread.
pub struct SqliteMigrationBackend {
    /// The connection the adapter owns, shared with the step it opens.
    pub(super) conn: rusqlite::Connection,
    lock_held: bool,
    lock_timeout: Duration,
}

impl SqliteMigrationBackend {
    /// Wraps an open SQLite connection.
    pub fn new(conn: rusqlite::Connection) -> Self {
        Self {
            conn,
            lock_held: false,
            lock_timeout: DEFAULT_SQLITE_LOCK_TIMEOUT,
        }
    }

    /// Overrides how long a run waits for the write lock before giving up.
    ///
    /// SQLite serialises writers through a single lock on the database file. A
    /// run that cannot take it within the timeout is rejected with a diagnostic
    /// naming the wait, so a runner blocked by a stuck peer fails fast instead
    /// of hanging forever.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Introspects the schema catalog of the wrapped connection.
    ///
    /// The catalog is read through the very introspection the rest of the
    /// toolchain uses, so the history table stays out of it and the reported
    /// types follow the SQLite affinity rules.
    pub fn introspect_catalog(&self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(&self.conn).map_err(backend_error)
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
        let mut statement = self
            .conn
            .prepare(&select_history_sql())
            .map_err(backend_error)?;

        let rows = statement
            .query_map([], |row| {
                Ok(RawHistoryRow {
                    installed_rank: row.get(0)?,
                    version: row.get(1)?,
                    description: row.get(2)?,
                    kind: row.get(3)?,
                    script: row.get(4)?,
                    checksum: row.get(5)?,
                    installed_on: row.get(6)?,
                    execution_time_ms: row.get(7)?,
                    success: row.get::<_, i64>(8)? != 0,
                })
            })
            .map_err(backend_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend_error)?;

        rows.into_iter().map(RawHistoryRow::decode).collect()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
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
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend_error)?;
        Ok(SqliteMigrationTransaction {
            step: Some(SqliteStep::Transaction(tx)),
        })
    }

    /// Executes a script marked non-transactional in autocommit mode.
    ///
    /// The run lock is held as an open `BEGIN IMMEDIATE` transaction, and the
    /// statements a script marks itself non-transactional for are exactly the
    /// ones SQLite rejects inside a transaction — `VACUUM`, several `PRAGMA`
    /// settings. The lock transaction is committed first, so the script runs
    /// outside any transaction the runner opened and every statement it
    /// completes is durable on its own. A script that fails midway therefore
    /// leaves its completed statements in the database: that is the documented
    /// contract of a non-transactional script, and the history row records the
    /// failure for a subsequent `repair`.
    ///
    /// Between the `COMMIT` and the re-take below the write lock is free:
    /// SQLite materialises its single-writer lock only as a transaction, so
    /// another runner may hold the lock for as long as its own run lasts. A
    /// runner that cannot re-take the lock keeps going without it — every
    /// following step opens its own `IMMEDIATE` transaction — and
    /// [`release_lock`](MigrationBackend::release_lock) tolerates the missing
    /// lock transaction.
    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        if self.lock_held {
            self.conn.execute_batch("COMMIT").map_err(backend_error)?;
            self.lock_held = false;
        }

        let result = self.conn.execute_batch(sql).map_err(backend_error);

        if !self.conn.is_autocommit() {
            // The script opened a transaction it never closed. The leftover
            // transaction is rolled back so the connection stays usable for
            // the failure record, and the diagnostic names the imbalance of
            // the script instead of the lock error a re-take would report.
            let _ = self.conn.execute_batch("ROLLBACK");
            return Err(Error::Backend(
                "non-transactional script left a transaction open; balance the BEGIN \
                 statements of a script marked non-transactional"
                    .to_owned(),
            ));
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
    /// SQLite materialises its single-writer lock as a transaction, so the
    /// lock is held as an open `BEGIN IMMEDIATE` until
    /// [`release_lock`](MigrationBackend::release_lock) commits it. A script
    /// marked non-transactional cannot run inside that transaction, so
    /// [`execute_non_transactional`](MigrationBackend::execute_non_transactional)
    /// commits the lock transaction before such a script and takes the lock
    /// again afterwards; between those statements another runner may hold
    /// the write lock for as long as its own run lasts.
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
        // A non-transactional script ends the lock transaction and a failed
        // re-take leaves the connection in autocommit, so a connection
        // holding no transaction has nothing to commit — the driver would
        // reject the statement with `cannot commit - no transaction is
        // active`.
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

/// One step of a SQLite migration run.
enum SqliteStep<'a> {
    /// A step that owns its native transaction.
    Transaction(rusqlite::Transaction<'a>),
    /// A step nested in the transaction that holds the run lock.
    Savepoint(&'a mut rusqlite::Connection),
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

    fn connection(&mut self) -> Result<&rusqlite::Connection, Error> {
        match self.current()? {
            SqliteStep::Transaction(tx) => Ok(&**tx),
            SqliteStep::Savepoint(conn) => Ok(&**conn),
        }
    }

    /// Introspects the schema catalog the step sees.
    ///
    /// The catalog is read over the connection of the step, so a migration
    /// observes the changes it has already applied without committing them.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(self.connection()?).map_err(backend_error)
    }

    fn run(&mut self, sql: &str, params: impl rusqlite::Params) -> Result<(), Error> {
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
        let sql = insert_entry_sql('?');
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());

        self.run(
            &sql,
            rusqlite::params![
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
        let sql = delete_entry_sql('?');
        self.run(&sql, rusqlite::params![installed_rank])
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql('?');
        self.run(&sql, rusqlite::params![checksum.to_hex(), script])
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
