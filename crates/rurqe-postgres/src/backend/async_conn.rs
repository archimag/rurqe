//! Asynchronous execution adapter over [`tokio_postgres::Client`].

use core::future::Future;

use futures_core::Stream;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery};
use tokio_postgres::types::ToSql;

use super::types::{rows_to_sql_vals, PgError, SqlValParam};

/// Executes `query` through an asynchronous client without decoding rows.
///
/// The helper accepts any [`tokio_postgres::GenericClient`] — a plain client, a
/// pooled client, or an open transaction — so the parameter binding logic is
/// shared by every asynchronous adapter.
pub(crate) async fn fetch_raw_async<C>(
    client: &C,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, PgError>
where
    C: tokio_postgres::GenericClient + Sync,
{
    // The driver takes its parameters as one slice of borrowed trait objects, so
    // the wrappers around the bound values are re-borrowed instead of their
    // payloads being copied into a buffer of their own.
    let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
    let bound: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|param| param as &(dyn ToSql + Sync))
        .collect();
    // The statement yields its whole result set at once, so every payload is
    // decoded straight from the borrowed row buffer.
    let rows = client.query(query.sql.as_str(), &bound).await?;
    rows_to_sql_vals(rows)
}

/// Executes `query` through an asynchronous client and returns affected rows.
pub(crate) async fn execute_raw_async<C>(client: &C, query: &BuiltQuery) -> Result<u64, PgError>
where
    C: tokio_postgres::GenericClient,
{
    let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
    let affected = client
        .execute_raw(
            query.sql.as_str(),
            params.iter().map(|param| param as &(dyn ToSql + Sync)),
        )
        .await?;
    Ok(affected)
}

/// Executes `query` through an asynchronous client and decodes at most `limit`
/// rows, stopping the row stream once the limit is reached.
///
/// Without a limit the whole result set is fetched, exactly as
/// [`fetch_raw_async`] does.
pub(crate) async fn fetch_raw_limited_async<C>(
    client: &C,
    query: &BuiltQuery,
    limit: Option<usize>,
) -> Result<Vec<Vec<SqlVal>>, PgError>
where
    C: tokio_postgres::GenericClient + Sync,
{
    match limit {
        None => fetch_raw_async(client, query).await,
        Some(limit) => {
            let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
            let stream = client
                .query_raw(
                    query.sql.as_str(),
                    params.iter().map(|param| param as &(dyn ToSql + Sync)),
                )
                .await?;
            rows_from_stream(stream, limit).await
        }
    }
}

/// Collects at most `limit` rows from the row stream of a `query_raw` call.
///
/// The stream is polled row by row and the loop stops as soon as `limit` rows
/// have been collected, so the remaining rows of an unbounded result set are
/// never read from the socket.
async fn rows_from_stream(
    stream: tokio_postgres::RowStream,
    limit: usize,
) -> Result<Vec<Vec<SqlVal>>, PgError> {
    let mut stream = std::pin::pin!(stream);
    let mut rows = Vec::new();
    while rows.len() < limit {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            Some(row) => rows.push(row?),
            None => break,
        }
    }
    rows_to_sql_vals(rows)
}

/// A [`tokio_postgres::Client`] executing RURQE queries asynchronously.
///
/// The adapter implements the asynchronous
/// [`AsyncQueryBackend`](rurqe_core::query::AsyncQueryBackend) contract, so the
/// `fetch_all`, `fetch_optional`, `fetch_one`, and `execute` helpers of
/// [`AsyncQueryBackendExt`](rurqe_core::query::AsyncQueryBackendExt) become
/// available on the connection without any adapter-local copy.
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

impl AsyncQueryBackend for PgAsyncConnection {
    type Error = PgError;

    fn fetch_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
        fetch_raw_async(&self.client, query)
    }

    fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
        fetch_raw_limited_async(&self.client, query, limit)
    }

    fn execute_raw(&self, query: &BuiltQuery) -> impl Future<Output = Result<u64, PgError>> + Send {
        execute_raw_async(&self.client, query)
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
///
/// The handle implements the same asynchronous
/// [`AsyncQueryBackend`](rurqe_core::query::AsyncQueryBackend) contract as the
/// connection, so query helpers behave identically inside and outside a
/// transaction.
pub struct PgAsyncTransaction<'a> {
    inner: tokio_postgres::Transaction<'a>,
}

impl PgAsyncTransaction<'_> {
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

impl AsyncQueryBackend for PgAsyncTransaction<'_> {
    type Error = PgError;

    fn fetch_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
        fetch_raw_async(&self.inner, query)
    }

    fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
        fetch_raw_limited_async(&self.inner, query, limit)
    }

    fn execute_raw(&self, query: &BuiltQuery) -> impl Future<Output = Result<u64, PgError>> + Send {
        execute_raw_async(&self.inner, query)
    }
}
