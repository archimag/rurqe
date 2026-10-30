//! The `rusqlite` connection adapter.

use core::ops::{Deref, DerefMut};

use ::rusqlite::Connection;

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{
    BuiltQuery, IsolationLevel, QueryBackend, SavepointName, TransactionBackend, TransactionGuard,
    TransactionStatus, TransactionalConnection,
};

use super::types::{value_ref_to_sql_val, SqlValParam};

/// A `rusqlite` connection that executes RURQE-built queries.
///
/// The adapter implements the RURQE
/// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
/// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
/// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available.
/// It also dereferences to the wrapped [`Connection`], so raw SQL can still be
/// executed directly.
pub struct SqliteConnection {
    connection: Connection,
    status: TransactionStatus<::rusqlite::Error>,
}

impl SqliteConnection {
    /// Wraps an existing `rusqlite` connection.
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            status: TransactionStatus::new(),
        }
    }

    /// Opens a new in-memory SQLite database.
    pub fn open_in_memory() -> Result<Self, ::rusqlite::Error> {
        Connection::open_in_memory().map(Self::new)
    }

    /// Returns a shared reference to the wrapped connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Returns a mutable reference to the wrapped connection.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Consumes the adapter, returning the wrapped connection.
    pub fn into_inner(self) -> Connection {
        self.connection
    }

    /// Runs `f` inside a transaction, committing on `Ok` and rolling back on `Err`.
    ///
    /// When no transaction is open the block runs between `BEGIN` and `COMMIT`.
    /// Calling `transaction` again while a transaction is already open nests the
    /// block inside a `SAVEPOINT`, so an inner failure rolls back only its own
    /// statements while the enclosing transaction stays intact. A panic raised by
    /// `f` is rolled back as well, so the transaction can never be left
    /// half-finished.
    pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T, E>,
        E: From<::rusqlite::Error>,
    {
        let mut guard = TransactionGuard::begin(self).map_err(E::from)?;
        let value = f(&mut guard)?;
        guard.commit().map_err(E::from)?;
        Ok(value)
    }
}

impl TransactionalConnection for SqliteConnection {
    fn transaction_status(&mut self) -> &mut TransactionStatus<::rusqlite::Error> {
        &mut self.status
    }
}

impl SqliteConnection {
    /// Answers a refused transaction-control statement as benign when the
    /// driver observes no open transaction.
    ///
    /// The raw handle of the adapter is a supported escape hatch, so a caller
    /// may issue `COMMIT` or `ROLLBACK` through it while a typed scope is
    /// still open. SQLite then refuses the statement the guard sends next with
    /// `cannot rollback - no transaction is active`, even though the
    /// connection is already exactly as clean as the guard wanted it to be, so
    /// that refusal must not be recorded as a poisoning failure. The
    /// observation is `is_autocommit` — the same one the CLI migration driver
    /// uses to detect a script that left a transaction open: a refusal raised
    /// while no transaction is open is benign, while one that leaves a
    /// transaction open stays the driver's own error.
    fn benign_if_closed(&self, error: ::rusqlite::Error) -> Result<(), ::rusqlite::Error> {
        if self.connection.is_autocommit() {
            Ok(())
        } else {
            Err(error)
        }
    }
}

impl TransactionBackend for SqliteConnection {
    fn begin(&mut self, _isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
        // SQLite serialises access through database-level locking, so the
        // requested isolation level does not map onto a `BEGIN` clause.
        self.connection.execute_batch("BEGIN")
    }

    /// Rolls the transaction back, tolerating a transaction the raw handle
    /// already closed.
    ///
    /// A refused `ROLLBACK` is benign exactly when the driver observes no open
    /// transaction, so the transaction bookkeeping records the connection
    /// clean instead of poisoning a healthy adapter. `commit` stays untouched:
    /// the ratified classification covers the rollback path only, so a
    /// desynchronized `COMMIT` keeps costing its own transaction call its
    /// driver error.
    fn rollback(&mut self) -> Result<(), Self::Error> {
        match self.connection.execute_batch("ROLLBACK") {
            Ok(()) => Ok(()),
            Err(error) => self.benign_if_closed(error),
        }
    }

    /// Rolls back to the savepoint `name`, tolerating a savepoint the raw
    /// handle already closed along with the transaction that carried it.
    ///
    /// The refusal is benign under the same observation as
    /// [`rollback`](TransactionBackend::rollback): no open transaction means
    /// there is no abandoned work left to report.
    fn rollback_to_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let statement = format!("ROLLBACK TO SAVEPOINT {}", name.as_str());
        match self.connection.execute_batch(&statement) {
            Ok(()) => Ok(()),
            Err(error) => self.benign_if_closed(error),
        }
    }

    /// Releases the savepoint `name`, tolerating a savepoint the raw handle
    /// already closed along with the transaction that carried it.
    ///
    /// The refusal is benign under the same observation as
    /// [`rollback`](TransactionBackend::rollback): a block whose savepoint is
    /// already gone holds no work the release could keep.
    fn release_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let statement = format!("RELEASE SAVEPOINT {}", name.as_str());
        match self.connection.execute_batch(&statement) {
            Ok(()) => Ok(()),
            Err(error) => self.benign_if_closed(error),
        }
    }
}

impl Deref for SqliteConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for SqliteConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

impl QueryBackend for SqliteConnection {
    type Error = ::rusqlite::Error;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        fetch_raw_with(&mut self.connection, query)
    }

    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        fetch_raw_limited_with(&mut self.connection, query, limit)
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        execute_raw_with(&mut self.connection, query)
    }
}

/// Fetches the rows of `query` through a raw `rusqlite` connection.
pub(super) fn fetch_raw_with(
    connection: &mut Connection,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, ::rusqlite::Error> {
    fetch_raw_limited_with(connection, query, None)
}

/// Fetches at most `limit` rows of `query` through a raw `rusqlite` connection.
///
/// The row loop stops as soon as `limit` rows have been read, so the remaining
/// rows of an unbounded result set are never stepped over or decoded.
pub(super) fn fetch_raw_limited_with(
    connection: &mut Connection,
    query: &BuiltQuery,
    limit: Option<usize>,
) -> Result<Vec<Vec<SqlVal>>, ::rusqlite::Error> {
    let mut statement = connection.prepare(&query.sql)?;
    let column_count = statement.column_count();
    let params = query.params.iter().map(SqlValParam);
    let mut rows = statement.query(::rusqlite::params_from_iter(params))?;

    let limit = limit.unwrap_or(usize::MAX);
    let mut result = Vec::new();
    while result.len() < limit {
        let Some(row) = rows.next()? else {
            break;
        };
        let mut values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            values.push(value_ref_to_sql_val(index, row.get_ref(index)?)?);
        }
        result.push(values);
    }
    Ok(result)
}

/// Executes `query` through a raw `rusqlite` connection.
pub(super) fn execute_raw_with(
    connection: &mut Connection,
    query: &BuiltQuery,
) -> Result<u64, ::rusqlite::Error> {
    let params = query.params.iter().map(SqlValParam);
    let affected = connection.execute(&query.sql, ::rusqlite::params_from_iter(params))?;
    Ok(affected as u64)
}
