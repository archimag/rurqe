//! The `rusqlite` connection adapter.

use core::ops::{Deref, DerefMut};

use ::rusqlite::Connection;

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{
    BuiltQuery, IsolationLevel, QueryBackend, TransactionBackend, TransactionGuard,
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
    status: TransactionStatus,
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
    fn transaction_status(&mut self) -> &mut TransactionStatus {
        &mut self.status
    }
}

impl TransactionBackend for SqliteConnection {
    fn begin(&mut self, _isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
        // SQLite serialises access through database-level locking, so the
        // requested isolation level does not map onto a `BEGIN` clause.
        self.connection.execute_batch("BEGIN")
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

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        execute_raw_with(&mut self.connection, query)
    }
}

/// Fetches the rows of `query` through a raw `rusqlite` connection.
pub(super) fn fetch_raw_with(
    connection: &mut Connection,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, ::rusqlite::Error> {
    let mut statement = connection.prepare(&query.sql)?;
    let column_count = statement.column_count();
    let params = query.params.iter().map(SqlValParam);
    let mut rows = statement.query(::rusqlite::params_from_iter(params))?;

    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
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
