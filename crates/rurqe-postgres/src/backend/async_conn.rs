//! Asynchronous execution adapter over [`tokio_postgres::Client`].

use core::future::Future;

use futures_core::Stream;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery, IsolationLevel};

use super::async_scope::{execute_control, PgAsyncScope, ScopeFuture};
use super::types::{rows_to_sql_vals, BoundParams, PgError};

/// Executes `query` through an asynchronous client without decoding rows.
///
/// The helper accepts any [`tokio_postgres::GenericClient`] — a plain client, a
/// pooled client, or an open transaction — so the parameter binding logic is
/// shared by every asynchronous adapter. The whole result set is drained; the
/// bounded form of the same call is [`fetch_raw_limited_async`].
pub(crate) async fn fetch_raw_async<C>(
    client: &C,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, PgError>
where
    C: tokio_postgres::GenericClient + Sync,
{
    fetch_raw_limited_async(client, query, None).await
}

/// Executes `query` through an asynchronous client and returns affected rows.
pub(crate) async fn execute_raw_async<C>(client: &C, query: &BuiltQuery) -> Result<u64, PgError>
where
    C: tokio_postgres::GenericClient,
{
    // The parameters are prepared once and handed over as references, so the
    // statement pays for one buffer of its own values and never for a copy of
    // them.
    let params = BoundParams::of(query);
    let affected = client
        .execute_raw(query.sql.as_str(), params.bindings())
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
    // The parameters are prepared once and handed over as references, so the
    // statement pays for one buffer of its own values and never for a copy of
    // them.
    let params = BoundParams::of(query);
    let stream = client
        .query_raw(query.sql.as_str(), params.bindings())
        .await?;
    rows_from_stream(stream, limit).await
}

/// Collects at most `limit` rows from the row stream of a `query_raw` call.
///
/// The stream is polled row by row and the loop stops as soon as `limit` rows
/// have been collected, so the remaining rows of a bounded result set are never
/// read from the socket. Without a limit the stream is drained to its end.
async fn rows_from_stream(
    stream: tokio_postgres::RowStream,
    limit: Option<usize>,
) -> Result<Vec<Vec<SqlVal>>, PgError> {
    let mut stream = std::pin::pin!(stream);
    let mut rows = Vec::new();
    let ceiling = limit.unwrap_or(usize::MAX);
    while rows.len() < ceiling {
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
    /// [`commit`](PgAsyncTransaction::commit) is called. Dropping the handle
    /// without committing requests a rollback through the driver's `Drop`, which
    /// is a fire-and-forget statement the driver neither awaits nor reports, so a
    /// cancelled or panicking block is rolled back only as far as the driver
    /// manages.
    ///
    /// The guard borrows the connection mutably for as long as it lives, so a
    /// second call while the first guard is open is rejected by the borrow
    /// checker, and the guard offers no savepoint nesting. The closure form
    /// [`transaction_with_isolation`](PgAsyncConnection::transaction_with_isolation)
    /// opens the transaction with an isolation level, nests through
    /// [`PgAsyncScope::transaction`], and awaits both the commit and the
    /// rollback.
    pub async fn transaction(&mut self) -> Result<PgAsyncTransaction<'_>, PgError> {
        tracing::info!("beginning transaction");
        let inner = self.client.transaction().await?;
        Ok(PgAsyncTransaction { inner })
    }

    /// Runs `f` inside a transaction at the requested `isolation` level,
    /// committing on `Ok` and rolling back on `Err`.
    ///
    /// The method mirrors [`transaction`](PgAsyncConnection::transaction) and
    /// differs in the statement that opens the block and in who closes it: the
    /// block runs between `BEGIN ISOLATION LEVEL <level>` and `COMMIT`, with the
    /// level text taken from `IsolationLevel::as_str`, and the statements of `f`
    /// execute through a [`PgAsyncScope`], so the adapter itself awaits the
    /// commit of a block that returned `Ok` and the rollback of a block that
    /// returned `Err`.
    ///
    /// ```text
    /// connection
    ///     .transaction_with_isolation(IsolationLevel::Serializable, |mut scope| {
    ///         Box::pin(async move {
    ///             insert_into::<Postgres, User>()
    ///                 .values((UserId::set(1i64),))
    ///                 .execute(&scope)
    ///                 .await?;
    ///
    ///             // A nested block is a savepoint: its rollback undoes only
    ///             // its own statements.
    ///             scope
    ///                 .transaction(|nested| {
    ///                     Box::pin(async move {
    ///                         insert_into::<Postgres, User>()
    ///                             .values((UserId::set(2i64),))
    ///                             .execute(&nested)
    ///                             .await?;
    ///                         Ok(())
    ///                     })
    ///                 })
    ///                 .await?;
    ///
    ///             Ok(())
    ///         })
    ///     })
    ///     .await?;
    /// ```
    ///
    /// The closure is erased into a boxed [`Send`] future, because an
    /// asynchronous terminal call returns an opaque `impl Future` that a closure
    /// return type cannot name on stable Rust. `E` is the error of `f` and has to
    /// absorb the driver error: the dialect error alias over `PgError` does, so a
    /// terminal call propagates with `?` unchanged inside the block.
    ///
    /// Both endings are awaited, so a block that returns `Err` has its statements
    /// rolled back before the error reaches the caller. A rollback the driver
    /// refuses is traced instead of replacing that error, and the connection must
    /// then be discarded rather than reused. A block that returns `Ok` while one
    /// of its statements was cancelled still commits the statements that
    /// completed, so a task that has to be atomic under cancellation lets the
    /// closure return the failure instead of dropping it.
    ///
    /// The method borrows the connection mutably, so no second transaction can
    /// start on it while the block runs; a nested block opens a savepoint through
    /// [`PgAsyncScope::transaction`] instead.
    pub async fn transaction_with_isolation<T, E, F>(
        &mut self,
        isolation: IsolationLevel,
        f: F,
    ) -> Result<T, E>
    where
        E: From<PgError>,
        F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
    {
        let client = &self.client;

        let mut begin = String::from("BEGIN ISOLATION LEVEL ");
        begin.push_str(isolation.as_str());
        execute_control(client, begin).await.map_err(E::from)?;

        match f(PgAsyncScope::new(client)).await {
            Ok(value) => {
                execute_control(client, String::from("COMMIT")).await.map_err(E::from)?;
                tracing::info!("committed transaction");
                Ok(value)
            }
            Err(error) => {
                let rolled_back = execute_control(client, String::from("ROLLBACK")).await;
                match rolled_back {
                    Ok(()) => tracing::warn!("rolled back transaction"),
                    Err(failure) => tracing::error!(
                        failure = %failure,
                        "rolling back the transaction failed; the connection must be discarded"
                    ),
                }
                Err(error)
            }
        }
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
/// [`commit`](PgAsyncTransaction::commit) persists the work, while dropping the
/// handle without calling it requests a rollback through the driver's own
/// `Drop`, which is a fire-and-forget statement the driver neither awaits nor
/// reports.
///
/// That drop-time rollback is best effort, so the guard alone cannot promise
/// that a cancelled block leaves nothing behind: `tokio-postgres` does not make
/// an in-flight statement cancel-safe, and a future dropped in the middle of a
/// statement can leave the connection waiting for a response, in which case the
/// rollback requested by the drop never reaches the server. The work of the
/// statements that completed stays pending in the open transaction until the
/// connection is discarded.
///
/// The guard borrows the connection mutably for as long as it lives and does not
/// nest: a second [`transaction`](PgAsyncConnection::transaction) call while the
/// first guard is alive is rejected by the borrow checker. A block that has to
/// select an isolation level, nest inside a savepoint, or have its rollback
/// awaited by the adapter belongs in
/// [`transaction_with_isolation`](PgAsyncConnection::transaction_with_isolation).
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
