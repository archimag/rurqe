//! Asynchronous execution adapter over [`tokio_postgres::Client`].

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery};

use super::types::{execute_raw_async, fetch_raw_async, PgError};

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

    async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_async(&self.client, query).await
    }

    async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        execute_raw_async(&self.client, query).await
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

    async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_async(&self.inner, query).await
    }

    async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        execute_raw_async(&self.inner, query).await
    }
}
