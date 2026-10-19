//! SQLite dialect implementation and runtime adapters for RURQE.
//!
//! The dialect is parameterised by a type-level engine version, so SQL features
//! introduced by newer SQLite releases exist only for versions that actually
//! support them:
//!
//! - [`Sqlite3_0`] — baseline dialect without `ON CONFLICT` or `RETURNING`.
//! - [`Sqlite3_24`] — adds `ON CONFLICT` upsert syntax.
//! - [`Sqlite3_35`] — adds the `RETURNING` clause.
//!
//! ```
//! use rurqe_core::dialect::Dialect;
//! use rurqe_sqlite::{Sqlite, Sqlite3_35};
//!
//! assert_eq!(Sqlite::<Sqlite3_35>::quote_identifier("users"), "\"users\"");
//! assert_eq!(Sqlite::<Sqlite3_35>::placeholder(1), "?");
//! ```

use core::marker::PhantomData;

pub use rurqe_core;
use rurqe_core::dialect::{Dialect, SupportsOnConflict, SupportsReturning};

/// Marker trait representing a SQLite engine version.
pub trait SqliteVersion: 'static + Send + Sync {}

/// Threshold trait fulfilled by versions at least as recent as `MAJOR.MINOR`.
///
/// Capability traits are implemented only for versions satisfying the threshold
/// that introduced them, which keeps unsupported SQL out of reach at compile time.
pub trait SqliteVersionAtLeast<const MAJOR: u32, const MINOR: u32>: SqliteVersion {}

/// SQLite 3.0 marker: the baseline dialect without upserts or returning clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_0;
impl SqliteVersion for Sqlite3_0 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_0 {}

/// SQLite 3.24 marker: introduces `ON CONFLICT` upsert syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_24;
impl SqliteVersion for Sqlite3_24 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_24 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_24 {}

/// SQLite 3.35 marker: introduces the `RETURNING` clause on DML statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_35;
impl SqliteVersion for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 35> for Sqlite3_35 {}

/// SQLite dialect parameterised by engine version (defaulting to [`Sqlite3_35`]).
///
/// Identifiers are double-quoted and escaped by doubling embedded quotes, while
/// bind parameters use anonymous `?` placeholders.
///
/// Versions below 3.24 do not implement [`SupportsOnConflict`]:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsOnConflict;
/// use rurqe_sqlite::{Sqlite, Sqlite3_0};
///
/// fn assert_on_conflict<D: SupportsOnConflict>() {}
/// assert_on_conflict::<Sqlite<Sqlite3_0>>();
/// ```
///
/// Versions below 3.35 do not implement [`SupportsReturning`]:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsReturning;
/// use rurqe_sqlite::{Sqlite, Sqlite3_24};
///
/// fn assert_returning<D: SupportsReturning>() {}
/// assert_returning::<Sqlite<Sqlite3_24>>();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite<V: SqliteVersion = Sqlite3_35>(PhantomData<fn() -> V>);

impl<V: SqliteVersion> Dialect for Sqlite<V> {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        for ch in ident.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    }

    fn push_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }
}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 35>> SupportsReturning for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 24>> SupportsOnConflict for Sqlite<V> {}

/// Re-export of the underlying `rusqlite` crate for adapter users.
#[cfg(feature = "rusqlite")]
pub use ::rusqlite;

/// A `rusqlite` connection extended with the RURQE execution contract.
#[cfg(feature = "rusqlite")]
pub use rusqlite_backend::SqliteConnection;

#[cfg(feature = "rusqlite")]
mod rusqlite_backend {
    use core::ops::{Deref, DerefMut};

    use ::rusqlite::types::{ToSql, ToSqlOutput, ValueRef};
    use ::rusqlite::Connection;

    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{
        BuiltQuery, IsolationLevel, QueryBackend, TransactionBackend, TransactionContext,
    };

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
        tx_context: TransactionContext,
        in_transaction: bool,
    }

    impl SqliteConnection {
        /// Wraps an existing `rusqlite` connection.
        pub fn new(connection: Connection) -> Self {
            Self {
                connection,
                tx_context: TransactionContext::new(),
                in_transaction: false,
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
            let outer = !self.in_transaction;
            let savepoint = if outer {
                tracing::info!("beginning transaction");
                self.begin(None).map_err(E::from)?;
                self.in_transaction = true;
                None
            } else {
                let name = self.tx_context.open_savepoint();
                tracing::debug!(savepoint = %name, "creating savepoint");
                self.savepoint(&name).map_err(E::from)?;
                Some(name)
            };

            let mut guard = TransactionGuard {
                connection: self,
                outer,
                savepoint,
                finished: false,
            };

            match f(&mut *guard.connection) {
                Ok(value) => {
                    if outer {
                        guard.connection.commit().map_err(E::from)?;
                        guard.connection.in_transaction = false;
                        tracing::info!("committed transaction");
                    } else if let Some(name) = guard.savepoint.as_deref() {
                        guard.connection.release_savepoint(name).map_err(E::from)?;
                        guard.connection.tx_context.close_savepoint();
                        tracing::debug!(savepoint = name, "released savepoint");
                    }
                    guard.finished = true;
                    Ok(value)
                }
                Err(error) => Err(error),
            }
        }
    }

    /// RAII guard that rolls the surrounding block back unless it finished cleanly.
    ///
    /// The guard owns the mutable borrow of the connection for the duration of the
    /// closure, so the closure still receives a reborrow while an unwind discards
    /// the block through [`Drop`].
    struct TransactionGuard<'a> {
        connection: &'a mut SqliteConnection,
        outer: bool,
        savepoint: Option<String>,
        finished: bool,
    }

    impl Drop for TransactionGuard<'_> {
        fn drop(&mut self) {
            if self.finished {
                return;
            }
            if self.outer {
                let _ = self.connection.rollback();
                self.connection.in_transaction = false;
                tracing::warn!("rolled back transaction");
            } else if let Some(savepoint) = self.savepoint.as_deref() {
                let _ = self.connection.rollback_to_savepoint(savepoint);
                let _ = self.connection.release_savepoint(savepoint);
                self.connection.tx_context.close_savepoint();
                tracing::warn!(savepoint = savepoint, "rolled back to savepoint");
            }
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

    /// Bridges a [`SqlVal`] into the `rusqlite` parameter interface.
    struct SqlValParam<'a>(&'a SqlVal);

    impl ToSql for SqlValParam<'_> {
        fn to_sql(&self) -> ::rusqlite::Result<ToSqlOutput<'_>> {
            match self.0 {
                SqlVal::Null => Ok(ToSqlOutput::Borrowed(ValueRef::Null)),
                SqlVal::I32(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(i64::from(*value)))),
                SqlVal::I64(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(*value))),
                SqlVal::F64(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Real(*value))),
                SqlVal::Bool(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(*value as i64))),
                SqlVal::Text(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Text(value.as_bytes()))),
                SqlVal::Bytes(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Blob(value.as_slice()))),
                #[allow(unreachable_patterns)]
                other => Ok(ToSqlOutput::Owned(::rusqlite::types::Value::Text(
                    other.to_string(),
                ))),
            }
        }
    }

    /// Decodes a raw SQLite value into the matching [`SqlVal`] variant.
    fn value_ref_to_sql_val(value: ValueRef<'_>) -> SqlVal {
        match value {
            ValueRef::Null => SqlVal::Null,
            ValueRef::Integer(value) => SqlVal::I64(value),
            ValueRef::Real(value) => SqlVal::F64(value),
            ValueRef::Text(bytes) => SqlVal::Text(String::from_utf8_lossy(bytes).into_owned()),
            ValueRef::Blob(bytes) => SqlVal::Bytes(bytes.to_vec()),
        }
    }

    pub(crate) fn fetch_raw_with(
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
                values.push(value_ref_to_sql_val(row.get_ref(index)?));
            }
            result.push(values);
        }
        Ok(result)
    }

    pub(crate) fn execute_raw_with(
        connection: &mut Connection,
        query: &BuiltQuery,
    ) -> Result<u64, ::rusqlite::Error> {
        let params = query.params.iter().map(SqlValParam);
        let affected = connection.execute(&query.sql, ::rusqlite::params_from_iter(params))?;
        Ok(affected as u64)
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
}

#[cfg(feature = "r2d2")]
mod r2d2_pool {
    use std::error::Error as StdError;
    use std::fmt;

    use r2d2::ManageConnection;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, QueryBackend};

    use crate::rusqlite_backend::{execute_raw_with, fetch_raw_with};

    /// An `r2d2` pool of SQLite connections.
    ///
    /// The pool implements the RURQE
    /// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
    /// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available
    /// directly on the pool: every call checks a connection out, runs the
    /// statement, and returns the connection to the pool.
    ///
    /// ```
    /// use r2d2_sqlite::SqliteConnectionManager;
    ///
    /// let manager = SqliteConnectionManager::memory();
    /// let pool = r2d2::Pool::builder().max_size(4).build(manager).unwrap();
    /// let _pool = rurqe_sqlite::SqlitePool::new(pool);
    /// ```
    pub struct SqlitePool<M: ManageConnection> {
        pool: r2d2::Pool<M>,
    }

    impl<M: ManageConnection<Connection = ::rusqlite::Connection>> SqlitePool<M> {
        /// Wraps an existing `r2d2` pool managing `rusqlite` connections.
        pub fn new(pool: r2d2::Pool<M>) -> Self {
            Self { pool }
        }

        /// Checks a connection out of the pool.
        pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error> {
            self.pool.get()
        }
    }

    /// Error raised while executing a query through an `r2d2` SQLite pool.
    #[derive(Debug)]
    pub enum SqlitePoolError {
        /// The connection could not be checked out of the pool.
        Pool(r2d2::Error),
        /// The statement failed to execute.
        Query(::rusqlite::Error),
    }

    impl fmt::Display for SqlitePoolError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                SqlitePoolError::Pool(error) => write!(f, "connection pool error: {error}"),
                SqlitePoolError::Query(error) => write!(f, "SQLite query error: {error}"),
            }
        }
    }

    impl StdError for SqlitePoolError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                SqlitePoolError::Pool(error) => Some(error),
                SqlitePoolError::Query(error) => Some(error),
            }
        }
    }

    impl<M: ManageConnection<Connection = ::rusqlite::Connection>> QueryBackend for SqlitePool<M> {
        type Error = SqlitePoolError;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut connection = self.pool.get().map_err(SqlitePoolError::Pool)?;
            fetch_raw_with(&mut *connection, query).map_err(SqlitePoolError::Query)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let mut connection = self.pool.get().map_err(SqlitePoolError::Pool)?;
            execute_raw_with(&mut *connection, query).map_err(SqlitePoolError::Query)
        }
    }
}

#[cfg(feature = "r2d2")]
pub use r2d2_pool::{SqlitePool, SqlitePoolError};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_standard_and_escaped_identifiers() {
        assert_eq!(Sqlite::<Sqlite3_35>::quote_identifier("users"), "\"users\"");
        assert_eq!(
            Sqlite::<Sqlite3_35>::quote_identifier("user\"name"),
            "\"user\"\"name\""
        );
        assert_eq!(
            Sqlite::<Sqlite3_0>::quote_identifier("select"),
            "\"select\""
        );
    }

    #[test]
    fn formats_anonymous_placeholders() {
        assert_eq!(Sqlite::<Sqlite3_35>::placeholder(1), "?");
        assert_eq!(Sqlite::<Sqlite3_35>::placeholder(42), "?");
        assert_eq!(Sqlite::<Sqlite3_0>::placeholder(7), "?");
    }

    #[test]
    fn capabilities_are_gated_by_version() {
        fn assert_returning<D: Dialect + SupportsReturning>() {}
        fn assert_on_conflict<D: Dialect + SupportsOnConflict>() {}

        assert_on_conflict::<Sqlite<Sqlite3_24>>();
        assert_on_conflict::<Sqlite<Sqlite3_35>>();

        assert_returning::<Sqlite<Sqlite3_35>>();
        assert_returning::<Sqlite>();
        assert_on_conflict::<Sqlite>();
    }
}
