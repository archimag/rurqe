//! The `r2d2` connection pool adapter.
//!
//! The pool carries the transactional guarantees of the connection-level
//! adapter to the pooled surface: `SqlitePool::transaction` keeps one
//! connection checked out for the whole block, so the guarded
//! `BEGIN`/`COMMIT`, the `SAVEPOINT` nesting of a block opened inside another
//! one, and the rollback of a failing or unwinding block behave exactly as they
//! do on an owned connection. The pooled handle itself returns to the pool on
//! every exit path, because the checkout guard is dropped however the block
//! ends. The session of a pooled connection outlives the block, so every
//! checkout returns it to an idle state before handing the connection out: a
//! transaction a previous block left open is rolled back there instead of
//! enclosing the next block.

use std::error::Error as StdError;
use std::fmt;

use r2d2::ManageConnection;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{BuiltQuery, QueryBackend};

use super::rusqlite::{
    ensure_idle, execute_raw_with, fetch_raw_limited_with, fetch_raw_with, SqliteConnection,
};

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
/// use rurqe_sqlite::{SqlitePool, SqlitePoolError};
///
/// # fn main() -> Result<(), SqlitePoolError> {
/// let manager = SqliteConnectionManager::memory();
/// let pool = r2d2::Pool::builder().max_size(4).build(manager)?;
/// let _pool = SqlitePool::new(pool);
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

// `r2d2::Pool` is an `Arc`-backed handle that clones without requiring the
// manager to be `Clone`, so the implementation is written out instead of
// derived: a derive would add an `M: Clone` bound to the handle.
impl<M: ManageConnection> Clone for SqlitePool<M> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

impl<M: ManageConnection<Connection = ::rusqlite::Connection>> SqlitePool<M> {
    /// Wraps an existing `r2d2` pool managing `rusqlite` connections.
    pub fn new(pool: r2d2::Pool<M>) -> Self {
        Self { pool }
    }

    /// Returns a shared reference to the wrapped pool.
    pub fn pool(&self) -> &r2d2::Pool<M> {
        &self.pool
    }

    /// Checks a connection out of the pool and returns it to an idle session.
    ///
    /// A checkout failure surfaces as [`SqlitePoolError::Pool`], so the whole
    /// pool API speaks one error vocabulary: a checkout failure and a
    /// statement failure both propagate with `?` into a function returning
    /// [`SqlitePoolError`].
    ///
    /// The connection of a recycled checkout may still hold a transaction a
    /// previous block left open, because `r2d2` validates that a connection is
    /// alive rather than that it is outside a transaction, so every checkout
    /// returns the session to an idle state before handing the connection out.
    /// The reset reads the engine's own observation of the session: a
    /// connection that is already outside a transaction issues no statement at
    /// all, and one that still holds a transaction has it ended by a single
    /// `ROLLBACK`.
    ///
    /// A reset the driver refuses means the session could not be brought back
    /// to a known state, so the checkout itself fails with
    /// [`SqlitePoolError::AbandonedTransaction`] instead of handing the
    /// connection to a caller that would run its work inside the abandoned
    /// transaction.
    pub fn get(&self) -> Result<r2d2::PooledConnection<M>, SqlitePoolError> {
        let mut connection = self.pool.get().map_err(SqlitePoolError::Pool)?;
        ensure_idle(&mut connection).map_err(SqlitePoolError::AbandonedTransaction)?;
        Ok(connection)
    }

    /// Runs `f` inside a transaction on a pooled connection, committing on
    /// `Ok` and rolling back on `Err`.
    ///
    /// The connection stays checked out for the whole block, so every statement
    /// of `f` runs on one physical connection and the block is atomic exactly
    /// as it is on an owned connection: the block runs between `BEGIN` and
    /// `COMMIT`, a call made while a transaction is already open nests inside a
    /// `SAVEPOINT` through [`SqliteConnection::transaction`], and a block
    /// returning `Err` or panicking is rolled back. The pooled handle returns
    /// to the pool however the block ends, because the checkout guard is
    /// dropped on every exit path.
    ///
    /// No isolation-level variant exists on purpose: SQLite serialises access
    /// through database-level locking, so the adapter's `BEGIN` ignores an
    /// isolation level by design.
    ///
    /// A checkout failure surfaces as [`SqlitePoolError::Pool`]; a failure
    /// raised by `BEGIN`, by `f`, or by `COMMIT` surfaces as
    /// [`SqlitePoolError::Query`].
    ///
    /// The block starts on a connection [`SqlitePool::get`] has returned to an
    /// idle session, so a transaction a previous checkout left open is closed
    /// before this block opens its own.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, SqlitePoolError>
    where
        F: FnOnce(
            &mut SqliteConnection<&mut ::rusqlite::Connection>,
        ) -> Result<T, ::rusqlite::Error>,
    {
        let mut pooled = self.get()?;
        SqliteConnection::borrowed(&mut pooled)
            .transaction(f)
            .map_err(SqlitePoolError::Query)
    }
}

/// Error raised while executing a query through an `r2d2` SQLite pool.
///
/// The enum is `#[non_exhaustive]`: it is the failure surface of the pool rather
/// than a closed set, so a match written against it carries a wildcard arm, and a
/// variant added later does not break the matches already compiled.
#[derive(Debug)]
#[non_exhaustive]
pub enum SqlitePoolError {
    /// The connection could not be checked out of the pool.
    Pool(r2d2::Error),
    /// The statement failed to execute.
    Query(::rusqlite::Error),
    /// The checked-out connection could not be returned to an idle session.
    AbandonedTransaction(::rusqlite::Error),
}

impl fmt::Display for SqlitePoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlitePoolError::Pool(error) => write!(f, "connection pool error: {error}"),
            SqlitePoolError::Query(error) => write!(f, "SQLite query error: {error}"),
            SqlitePoolError::AbandonedTransaction(error) => {
                write!(f, "abandoned transaction: {error}")
            }
        }
    }
}

impl StdError for SqlitePoolError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            SqlitePoolError::Pool(error) => Some(error),
            SqlitePoolError::Query(error) => Some(error),
            SqlitePoolError::AbandonedTransaction(error) => Some(error),
        }
    }
}

impl From<r2d2::Error> for SqlitePoolError {
    fn from(error: r2d2::Error) -> Self {
        SqlitePoolError::Pool(error)
    }
}

impl From<::rusqlite::Error> for SqlitePoolError {
    fn from(error: ::rusqlite::Error) -> Self {
        SqlitePoolError::Query(error)
    }
}

impl<M: ManageConnection<Connection = ::rusqlite::Connection>> QueryBackend for SqlitePool<M> {
    type Error = SqlitePoolError;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        let mut connection = self.get()?;
        fetch_raw_with(&mut connection, query).map_err(SqlitePoolError::Query)
    }

    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        let mut connection = self.get()?;
        fetch_raw_limited_with(&mut connection, query, limit).map_err(SqlitePoolError::Query)
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        let mut connection = self.get()?;
        execute_raw_with(&mut connection, query).map_err(SqlitePoolError::Query)
    }
}

#[cfg(test)]
mod tests {
    use r2d2_sqlite::SqliteConnectionManager;

    use super::{SqlitePool, SqlitePoolError};

    /// Builds a one-connection pool over a shared in-memory database.
    fn pool() -> SqlitePool<SqliteConnectionManager> {
        // A single connection keeps the in-memory database alive across
        // checkouts, so every block observes the work the previous one
        // committed.
        let pool = r2d2::Pool::builder()
            .max_size(1)
            .build(SqliteConnectionManager::memory())
            .expect("a one-connection in-memory pool builds");
        SqlitePool::new(pool)
    }

    #[test]
    fn transaction_commits_the_block_and_returns_the_connection() {
        let pool = pool();

        pool.transaction(|tx| {
            tx.execute_batch(
                "CREATE TABLE account (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL)",
            )
        })
        .expect("the schema block commits");

        pool.transaction(|tx| tx.execute_batch("INSERT INTO account (balance) VALUES (10)"))
            .expect("the committed block returns its connection");

        let rolled_back = pool.transaction(|tx| {
            tx.execute_batch("INSERT INTO account (balance) VALUES (20)")?;
            Err::<(), _>(::rusqlite::Error::InvalidQuery)
        });
        assert!(
            rolled_back.is_err(),
            "a failing block reports its own error"
        );

        let count = pool
            .transaction(|tx| {
                tx.query_row(
                    "SELECT count(*) FROM account",
                    ::rusqlite::params![],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("the counting block commits");
        assert_eq!(count, 1, "the rolled back insert stayed out of the table");
    }

    #[test]
    fn nested_block_rolls_back_to_its_savepoint() {
        let pool = pool();

        pool.transaction(|tx| {
            tx.execute_batch(
                "CREATE TABLE account (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL)",
            )
        })
        .expect("the schema block commits");

        pool.transaction(|tx| {
            tx.execute_batch("INSERT INTO account (balance) VALUES (10)")?;

            let nested = tx.transaction(|inner| {
                inner.execute_batch("INSERT INTO account (balance) VALUES (20)")?;
                Err::<(), _>(::rusqlite::Error::InvalidQuery)
            });
            assert!(nested.is_err(), "the nested block reports its own error");

            Ok(())
        })
        .expect("the enclosing block commits");

        let count = pool
            .transaction(|tx| {
                tx.query_row(
                    "SELECT count(*) FROM account",
                    ::rusqlite::params![],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("the counting block commits");
        assert_eq!(count, 1, "only the enclosing insert survived");
    }

    #[test]
    fn the_pool_error_absorbs_the_driver_error() {
        let failure = SqlitePoolError::from(::rusqlite::Error::InvalidQuery);

        assert!(matches!(failure, SqlitePoolError::Query(_)));
    }

    #[test]
    fn a_refused_reset_keeps_the_driver_failure() {
        let failure = SqlitePoolError::AbandonedTransaction(::rusqlite::Error::InvalidQuery);

        assert!(
            matches!(&failure, SqlitePoolError::AbandonedTransaction(_)),
            "the checkout refuses the connection instead of reporting a query failure"
        );
        assert!(
            std::error::Error::source(&failure).is_some(),
            "the driver failure stays reachable through the checkout error"
        );
    }

    #[test]
    fn a_cloned_handle_checks_a_connection_out() {
        let pool = pool();
        let clone = pool.clone();

        assert!(clone.get().is_ok(), "the clone serves a connection");
    }
}
