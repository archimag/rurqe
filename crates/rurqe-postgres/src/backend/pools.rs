//! Connection pool integrations for PostgreSQL drivers.

#[cfg(feature = "deadpool-postgres")]
mod deadpool_impl {
    use core::future::Future;

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

        fn fetch_raw(
            &self,
            query: &BuiltQuery,
        ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
            async move {
                let client = self.pool.get().await?;
                fetch_raw_async(&*client, query).await
            }
        }

        fn fetch_raw_limited(
            &self,
            query: &BuiltQuery,
            limit: Option<usize>,
        ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
            async move {
                let client = self.pool.get().await?;
                fetch_raw_limited_async(&*client, query, limit).await
            }
        }

        fn execute_raw(
            &self,
            query: &BuiltQuery,
        ) -> impl Future<Output = Result<u64, PgError>> + Send {
            async move {
                let client = self.pool.get().await?;
                execute_raw_async(&*client, query).await
            }
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
    use rurqe_core::query::{BuiltQuery, QueryBackend};

    use crate::backend::sync_conn::{execute_raw_sync, fetch_raw_limited_sync, fetch_raw_sync};
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
