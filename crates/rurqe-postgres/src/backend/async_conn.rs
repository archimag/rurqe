//! Asynchronous execution adapter over [`tokio_postgres::Client`].

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{BuiltQuery, ExecutableQuery, ExecutionError};
use rurqe_core::row::FromRow;

use super::types::{execute_raw_async, fetch_all_async, fetch_raw_async, PgError};

/// A [`tokio_postgres::Client`] executing RURQE queries asynchronously.
///
/// The adapter mirrors the synchronous execution helpers of
/// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) with `async`
/// methods: [`fetch_all`](PgAsyncConnection::fetch_all),
/// [`fetch_optional`](PgAsyncConnection::fetch_optional),
/// [`fetch_one`](PgAsyncConnection::fetch_one), and
/// [`execute`](PgAsyncConnection::execute).
pub struct PgAsyncConnection {
    client: tokio_postgres::Client,
}

impl PgAsyncConnection {
    /// Wraps an existing asynchronous PostgreSQL client.
    pub fn new(client: tokio_postgres::Client) -> Self {
        Self { client }
    }

    /// Returns a shared reference to the wrapped client.
    pub fn client(&self) -> &tokio_postgres::Client {
        &self.client
    }

    /// Executes `query` and returns every produced row as ordered values.
    pub async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_async(&self.client, query).await
    }

    /// Executes `query` without decoding rows and returns the affected rows.
    pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        execute_raw_async(&self.client, query).await
    }

    /// Executes `query` and decodes every produced row.
    pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        fetch_all_async(&self.client, query).await
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

    /// Begins a transaction over the connection.
    ///
    /// The returned [`PgAsyncTransaction`] issues `COMMIT` only when
    /// [`commit`](PgAsyncTransaction::commit) is called and rolls the
    /// transaction back when it is dropped, so a returned error, a panic, and
    /// a cancelled future all leave the connection without an open
    /// transaction.
    pub async fn transaction(&mut self) -> Result<PgAsyncTransaction<'_>, PgError> {
        tracing::info!("beginning transaction");
        let inner = self.client.transaction().await?;
        Ok(PgAsyncTransaction { inner })
    }
}

/// An open transaction started by [`PgAsyncConnection::transaction`].
///
/// The wrapper runs the query helpers on the transactional connection and
/// keeps the transaction alive across `await` points. Committing is explicit:
/// dropping the handle without calling [`commit`](PgAsyncTransaction::commit)
/// rolls the transaction back, so a cancelled future or a panicking block
/// never leaves partial work behind — the driver issues the `ROLLBACK` from
/// `Drop` without awaiting anything.
pub struct PgAsyncTransaction<'a> {
    inner: tokio_postgres::Transaction<'a>,
}

impl PgAsyncTransaction<'_> {
    /// Executes `query` and returns every produced row as ordered values.
    pub async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_async(&self.inner, query).await
    }

    /// Executes `query` without decoding rows and returns the affected rows.
    pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        execute_raw_async(&self.inner, query).await
    }

    /// Executes `query` and decodes every produced row.
    pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        fetch_all_async(&self.inner, query).await
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

    /// Commits the transaction, persisting all its work.
    pub async fn commit(self) -> Result<(), PgError> {
        self.inner.commit().await?;
        tracing::info!("committed transaction");
        Ok(())
    }

    /// Rolls the transaction back, discarding all its work.
    pub async fn rollback(self) -> Result<(), PgError> {
        self.inner.rollback().await?;
        tracing::warn!("rolled back transaction");
        Ok(())
    }
}
