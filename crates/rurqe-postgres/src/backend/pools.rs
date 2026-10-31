//! Connection pool integrations for PostgreSQL drivers.
//!
//! The synchronous `PgPool` carries the transactional guarantees of the
//! connection-level adapter to the pooled surface: `PgPool::transaction` and
//! `PgPool::transaction_with_isolation` keep one connection checked out for the
//! whole block, so the guarded `BEGIN`/`COMMIT`, the `SAVEPOINT` nesting of a
//! block opened inside another one, the selected isolation level, and the
//! rollback of a failing or unwinding block behave exactly as they do on an
//! owned connection. The pooled handle itself returns to the pool on every exit
//! path, because the checkout guard is dropped however the block ends.

#[cfg(feature = "deadpool-postgres")]
mod deadpool_impl {
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{AsyncQueryBackend, BuiltQuery};

    use crate::backend::async_conn::{execute_raw_async, fetch_raw_async, fetch_raw_limited_async};
    use crate::backend::types::PgError;

    /// A `deadpool-postgres` pool executing RURQE queries asynchronously.
    ///
    /// The pool implements the asynchronous
    /// [`AsyncQueryBackend`](rurqe_core::query::AsyncQueryBackend) contract, so
    /// the `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`AsyncQueryBackendExt`](rurqe_core::query::AsyncQueryBackendExt) become
    /// available directly on the pool: every call checks a connection out,
    /// awaits the statement, and returns the connection to the pool.
    #[derive(Clone)]
    pub struct PgAsyncPool {
        pool: deadpool_postgres::Pool,
    }

    impl PgAsyncPool {
        /// Wraps an existing `deadpool-postgres` pool.
        pub fn new(pool: deadpool_postgres::Pool) -> Self {
            Self { pool }
        }

        /// Returns a shared reference to the wrapped pool.
        pub fn pool(&self) -> &deadpool_postgres::Pool {
            &self.pool
        }
    }

    impl AsyncQueryBackend for PgAsyncPool {
        type Error = PgError;

        async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let client = self.pool.get().await?;
            fetch_raw_async(&**client, query).await
        }

        /// Reads at most `limit` rows from the checked-out connection.
        ///
        /// The limit reaches the row stream of the underlying
        /// `tokio_postgres` client, so a single-row lookup stops after the
        /// second row instead of materialising the whole result set.
        async fn fetch_raw_limited(
            &self,
            query: &BuiltQuery,
            limit: Option<usize>,
        ) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let client = self.pool.get().await?;
            fetch_raw_limited_async(&**client, query, limit).await
        }

        async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            let client = self.pool.get().await?;
            execute_raw_async(&**client, query).await
        }
    }
}

#[cfg(feature = "deadpool-postgres")]
pub use deadpool_impl::PgAsyncPool;

#[cfg(feature = "r2d2")]
mod r2d2_impl {
    use std::error::Error as StdError;
    use std::fmt;

    use r2d2::ManageConnection;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, IsolationLevel, QueryBackend};

    use crate::backend::sync_conn::{
        execute_raw_sync, fetch_raw_limited_sync, fetch_raw_sync, PgConnection,
    };
    use crate::backend::types::PgError;

    /// An `r2d2` pool of synchronous PostgreSQL connections.
    ///
    /// The pool implements the RURQE
    /// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
    /// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available
    /// directly on the pool: every call checks a connection out, runs the
    /// statement, and returns the connection to the pool.
    pub struct PgPool<M: ManageConnection> {
        pool: r2d2::Pool<M>,
    }

    impl<M: ManageConnection<Connection = postgres::Client>> PgPool<M> {
        /// Wraps an existing `r2d2` pool managing `postgres` connections.
        pub fn new(pool: r2d2::Pool<M>) -> Self {
            Self { pool }
        }

        /// Checks a connection out of the pool.
        pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error> {
            self.pool.get()
        }

        /// Runs `f` inside a transaction on a pooled connection, committing on
        /// `Ok` and rolling back on `Err`.
        ///
        /// The connection stays checked out for the whole block, so every
        /// statement of `f` runs on one physical connection and the block is
        /// atomic exactly as it is on an owned connection: the outermost call
        /// runs between `BEGIN` and `COMMIT`, a call made while a transaction
        /// is already open nests inside a `SAVEPOINT` through
        /// [`PgConnection::transaction`], and a block returning `Err` or
        /// panicking is rolled back. The pooled handle returns to the pool
        /// however the block ends, because the checkout guard is dropped on
        /// every exit path.
        ///
        /// A checkout failure surfaces as [`PgPoolError::Pool`]; a failure
        /// raised by `BEGIN`, by `f`, or by `COMMIT` surfaces as
        /// [`PgPoolError::Query`].
        pub fn transaction<T, F>(&self, f: F) -> Result<T, PgPoolError>
        where
            F: FnOnce(&mut PgConnection<&mut postgres::Client>) -> Result<T, PgError>,
        {
            let mut pooled = self.pool.get().map_err(PgPoolError::Pool)?;
            PgConnection::borrowed(&mut *pooled)
                .transaction(f)
                .map_err(PgPoolError::Query)
        }

        /// Runs `f` inside a transaction at the requested `isolation` level,
        /// committing on `Ok` and rolling back on `Err`.
        ///
        /// The helper mirrors [`PgPool::transaction`] and differs only in the
        /// statement that opens the outermost block: the block runs between
        /// `BEGIN ISOLATION LEVEL <level>` and `COMMIT`, with the level text
        /// coming from [`IsolationLevel::as_str`], so every statement of `f`
        /// observes the isolation the caller asked for. A call made while a
        /// transaction is already open nests inside a `SAVEPOINT` and the
        /// requested level is ignored for that nested block, because SQL
        /// defines no per-savepoint isolation and the enclosing transaction
        /// keeps the level it was opened with.
        ///
        /// The error mapping is the same as for [`PgPool::transaction`]: a
        /// checkout failure surfaces as [`PgPoolError::Pool`], while a failure
        /// raised by `BEGIN`, by `f`, or by `COMMIT` surfaces as
        /// [`PgPoolError::Query`].
        pub fn transaction_with_isolation<T, F>(
            &self,
            isolation: IsolationLevel,
            f: F,
        ) -> Result<T, PgPoolError>
        where
            F: FnOnce(&mut PgConnection<&mut postgres::Client>) -> Result<T, PgError>,
        {
            let mut pooled = self.pool.get().map_err(PgPoolError::Pool)?;
            PgConnection::borrowed(&mut *pooled)
                .transaction_with_isolation(isolation, f)
                .map_err(PgPoolError::Query)
        }
    }

    /// Error raised while executing a query through an `r2d2` PostgreSQL pool.
    #[derive(Debug)]
    pub enum PgPoolError {
        /// The connection could not be checked out of the pool.
        Pool(r2d2::Error),
        /// The statement failed to execute.
        Query(PgError),
    }

    impl fmt::Display for PgPoolError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PgPoolError::Pool(error) => write!(f, "connection pool error: {error}"),
                PgPoolError::Query(error) => write!(f, "{error}"),
            }
        }
    }

    impl StdError for PgPoolError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                PgPoolError::Pool(error) => Some(error),
                PgPoolError::Query(error) => Some(error),
            }
        }
    }

    impl<M: ManageConnection<Connection = postgres::Client>> QueryBackend for PgPool<M> {
        type Error = PgPoolError;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            fetch_raw_sync(&mut *connection, query).map_err(PgPoolError::Query)
        }

        /// Reads at most `limit` rows from the checked-out connection.
        ///
        /// The limit reaches the row iterator of the underlying `postgres`
        /// client, so a single-row lookup stops after the second row instead of
        /// materialising the whole result set.
        fn fetch_raw_limited(
            &mut self,
            query: &BuiltQuery,
            limit: Option<usize>,
        ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            fetch_raw_limited_sync(&mut *connection, query, limit).map_err(PgPoolError::Query)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            execute_raw_sync(&mut *connection, query).map_err(PgPoolError::Query)
        }
    }
}

#[cfg(feature = "r2d2")]
pub use r2d2_impl::{PgPool, PgPoolError};
