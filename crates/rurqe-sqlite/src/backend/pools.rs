//! The `r2d2` connection pool adapter.

use std::error::Error as StdError;
use std::fmt;

use r2d2::ManageConnection;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{BuiltQuery, QueryBackend};

use super::rusqlite::{execute_raw_with, fetch_raw_with};

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
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let manager = SqliteConnectionManager::memory();
/// let pool = r2d2::Pool::builder().max_size(4).build(manager)?;
/// let _pool = rurqe_sqlite::SqlitePool::new(pool);
/// # Ok(())
/// # }
/// ```
///
/// The manager comes from the `r2d2_sqlite` development dependency, so the
/// example is compiled and run by the crate's documentation tests whenever the
/// `r2d2` feature is enabled.
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
