//! Connection pool integrations for PostgreSQL drivers.

#[cfg(feature = "deadpool-postgres")]
mod deadpool_impl {
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, ExecutableQuery, ExecutionError};
    use rurqe_core::row::FromRow;

    use crate::backend::types::{execute_raw_async, fetch_all_async, fetch_raw_async, PgError};

    /// A `deadpool-postgres` pool executing RURQE queries asynchronously.
    ///
    /// Each method checks a connection out of the pool, awaits the statement, and
    /// returns the connection, so the helpers mirror the asynchronous methods of
    /// [`PgAsyncConnection`](crate::PgAsyncConnection) without manual extraction.
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

        /// Executes `query` and returns every produced row as ordered values.
        pub async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let client = self.pool.get().await.map_err(PgError::Pool)?;
            fetch_raw_async(&*client, query).await
        }

        /// Executes `query` without decoding rows and returns the affected rows.
        pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            let client = self.pool.get().await.map_err(PgError::Pool)?;
            execute_raw_async(&*client, query).await
        }

        /// Executes `query` and decodes every produced row.
        pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let client = self
                .pool
                .get()
                .await
                .map_err(|error| ExecutionError::Backend(PgError::Pool(error)))?;
            fetch_all_async(&*client, query).await
        }

        /// Executes `query` and decodes at most one row.
        pub async fn fetch_optional<Q>(
            &self,
            query: &Q,
        ) -> Result<Option<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let rows = self.fetch_all(query).await?;
            match rows.len() {
                0 => Ok(None),
                1 => Ok(rows.into_iter().next()),
                count => Err(ExecutionError::MultipleRows { count }),
            }
        }

        /// Executes `query` and decodes exactly one row.
        pub async fn fetch_one<Q>(&self, query: &Q) -> Result<Q::Row, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            self.fetch_optional(query)
                .await?
                .ok_or(ExecutionError::NoRows)
        }

        /// Executes `query` and returns the number of affected rows.
        pub async fn execute<Q>(&self, query: &Q) -> Result<u64, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
        {
            let built = query.build_query();
            self.execute_raw(&built)
                .await
                .map_err(ExecutionError::Backend)
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

    use crate::backend::types::{execute_raw_sync, fetch_raw_sync, PgError};

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

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            execute_raw_sync(&mut *connection, query).map_err(PgPoolError::Query)
        }
    }
}

#[cfg(feature = "r2d2")]
pub use r2d2_impl::{PgPool, PgPoolError};
