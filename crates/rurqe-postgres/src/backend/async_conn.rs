//! Asynchronous execution adapter over [`tokio_postgres::Client`].
//!
//! The adapter tracks the transaction state of the connection it drives and
//! poisons it whenever it cannot prove that the connection is outside a
//! transaction: a `COMMIT` or a `ROLLBACK` the driver refused, a `BEGIN` that
//! failed, a panicking block, or a block whose future was dropped before either
//! control statement ran. A poisoned connection refuses every statement and every
//! transaction with [`PgError::ConnectionPoisoned`] instead of executing them
//! inside the abandoned transaction, which PostgreSQL would accept with a warning
//! and persist with the next `COMMIT`. The state is sticky and is never cleared:
//! the owner of the handle discards it, and the abandoned transaction closes when
//! the socket of the discarded connection closes.
use core::future::Future;
use std::sync::Arc;

use futures_core::Stream;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery, IsolationLevel};

use super::async_scope::{execute_control, PgAsyncScope, ScopeFuture};
use super::poison::{control_failure, BlockExitGuard, PoisonCell};
use super::types::{BoundParams, PgError, RowDecoder};

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
/// rows, stopping the poll loop once the limit is reached.
///
/// Without a limit the whole result set is fetched, exactly as
/// [`fetch_raw_async`] does.
///
/// The bound saves decoding and materialisation, never server work. The wire
/// protocol requires the response of a statement to be consumed through
/// `ReadyForQuery` before the connection can read the response of the next
/// command, so the driver keeps reading the `DataRow` messages past the limit
/// and discards them. The next statement on that connection therefore
/// completes only after the drain, which matters for a pooled connection: an
/// asynchronous pooled handle (`PgAsyncPool`) returns to the pool as soon as
/// the fetch completes, and the caller that checks the connection out next
/// waits for the drain. Truncating the work the server performs would take an
/// explicit cursor (`DECLARE` / `FETCH`), which this adapter does not open.
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
/// have been collected, so a row past the limit is never decoded or
/// materialised. Without a limit the stream is drained to its end.
///
/// Stopping the loop does not stop the statement: the server has already
/// produced the rows it returns, and the wire protocol requires the response to
/// be consumed through `ReadyForQuery` before the connection can read the
/// response of the next command. The driver therefore reads the unread
/// `DataRow` messages of a bounded result set and discards them, so the next
/// statement on that connection completes only after the drain.
///
/// Every row is decoded as it arrives instead of being collected first, so the
/// message buffer of a row is released before the next one is read.
async fn rows_from_stream(
    stream: tokio_postgres::RowStream,
    limit: Option<usize>,
) -> Result<Vec<Vec<SqlVal>>, PgError> {
    let mut stream = std::pin::pin!(stream);
    let mut decoder = RowDecoder::new();
    let mut rows = Vec::new();
    let ceiling = limit.unwrap_or(usize::MAX);
    while rows.len() < ceiling {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            Some(row) => {
                let row = row?;
                rows.push(decoder.decode(&row)?);
            }
            None => break,
        }
    }
    Ok(rows)
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
    /// Poison record shared with the transaction handles of this connection.
    poison: Arc<PoisonCell>,
}

impl PgAsyncConnection {
    /// Wraps an existing asynchronous PostgreSQL client.
    pub fn new(client: tokio_postgres::Client) -> Self {
        Self {
            client,
            poison: Arc::new(PoisonCell::new()),
        }
    }

    /// Returns a shared reference to the wrapped client.
    pub fn client(&self) -> &tokio_postgres::Client {
        &self.client
    }

    /// Reports whether the connection is poisoned and must be discarded.
    ///
    /// The state is recorded when the adapter can no longer prove that the
    /// connection is outside a transaction: a transaction-control statement the
    /// driver refused, or a block that ended without one. Every further statement
    /// and every further transaction is refused with
    /// [`PgError::ConnectionPoisoned`], so the caller detects the state here and
    /// drops the handle instead of running work inside an abandoned transaction.
    pub fn is_poisoned(&self) -> bool {
        self.poison.is_poisoned()
    }

    /// Refuses to run anything on a poisoned connection.
    ///
    /// The refusal carries the failure that produced the state, so the caller
    /// learns which transaction-control statement could not be completed instead
    /// of only that the connection is unusable.
    fn refuse_if_poisoned(&self) -> Result<(), PgError> {
        if !self.poison.is_poisoned() {
            return Ok(());
        }

        let reason = self
            .poison
            .poison_reason()
            .unwrap_or_else(|| String::from("the connection recorded no reason"));
        Err(PgError::ConnectionPoisoned { reason })
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
    ///
    /// The entry refuses to open a transaction on a poisoned connection: the handle
    /// is created only while the adapter can prove that no abandoned transaction is
    /// open, and a refused entry returns [`PgError::ConnectionPoisoned`] carrying
    /// the recorded reason. A `COMMIT` or a `ROLLBACK` the driver refuses on the
    /// returned handle poisons the connection in turn, so the failure stays
    /// observable after the handle is consumed.
    pub async fn transaction(&mut self) -> Result<PgAsyncTransaction<'_>, PgError> {
        self.refuse_if_poisoned()?;
        let poison = Arc::clone(&self.poison);
        tracing::info!("beginning transaction");
        let inner = self.client.transaction().await?;
        Ok(PgAsyncTransaction { inner, poison })
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
    /// rolled back before the error reaches the caller. A `COMMIT` the driver
    /// refuses returns its own failure, while a `ROLLBACK` it refuses keeps the
    /// error of the block; in both cases the connection is poisoned, because the
    /// transaction state is unknown from that point on. A block that returns `Ok`
    /// while one of its statements was cancelled still commits the statements that
    /// completed, so a task that has to be atomic under cancellation lets the
    /// closure return the failure instead of dropping it.
    ///
    /// # Poisoning
    ///
    /// Every ending the adapter cannot prove clean is recorded in the poison cell
    /// the connection shares with its transaction handles.
    /// [`PgAsyncConnection::is_poisoned`] reports the state, and a poisoned
    /// connection refuses every transaction and every statement with
    /// [`PgError::ConnectionPoisoned`], carrying the failure that produced it.
    /// Besides a refused `COMMIT` or `ROLLBACK`, a failed `BEGIN`, a block that
    /// panics, and a block whose future is dropped before either statement is
    /// awaited poison the connection: in the last two cases the transaction the
    /// block opened stays live on the server, and PostgreSQL accepts the `BEGIN` of
    /// a later caller inside it with a warning instead of refusing it, so anything
    /// executed next would join the abandoned transaction and persist it with the
    /// next `COMMIT`.
    ///
    /// The record is never cleared: a poisoned connection is discarded by its
    /// owner, and the abandoned transaction closes when the socket of the discarded
    /// connection closes, because closing it needs an awaited driver call that a
    /// drop cannot issue. Statements issued through the raw client handed out by
    /// [`client`](PgAsyncConnection::client) are outside this bookkeeping, exactly
    /// like the raw driver handles of the synchronous adapters.
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
        self.refuse_if_poisoned().map_err(E::from)?;

        let client = &self.client;
        let poison = Arc::clone(&self.poison);
        let mut guard = BlockExitGuard::new(Arc::clone(&poison));

        let mut begin = String::from("BEGIN ISOLATION LEVEL ");
        begin.push_str(isolation.as_str());
        if let Err(failure) = execute_control(client, begin).await {
            poison.mark(control_failure("BEGIN", &failure));
            return Err(E::from(failure));
        }

        match f(PgAsyncScope::new(client)).await {
            Ok(value) => match execute_control(client, String::from("COMMIT")).await {
                Ok(()) => {
                    guard.mark_completed();
                    tracing::info!("committed transaction");
                    Ok(value)
                }
                Err(failure) => {
                    poison.mark(control_failure("COMMIT", &failure));
                    Err(E::from(failure))
                }
            },
            Err(error) => {
                match execute_control(client, String::from("ROLLBACK")).await {
                    Ok(()) => {
                        guard.mark_completed();
                        tracing::warn!("rolled back transaction");
                    }
                    Err(failure) => {
                        poison.mark(control_failure("ROLLBACK", &failure));
                        tracing::error!(
                            failure = %failure,
                            "rolling back the transaction failed; the connection is \
                             poisoned and must be discarded"
                        );
                    }
                }
                Err(error)
            }
        }
    }
}

impl AsyncQueryBackend for PgAsyncConnection {
    type Error = PgError;

    async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        self.refuse_if_poisoned()?;
        fetch_raw_async(&self.client, query).await
    }

    async fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, PgError> {
        self.refuse_if_poisoned()?;
        fetch_raw_limited_async(&self.client, query, limit).await
    }

    async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        self.refuse_if_poisoned()?;
        execute_raw_async(&self.client, query).await
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
/// The handle shares the poison record of the connection that opened it, so a
/// `COMMIT` or a `ROLLBACK` the driver refuses poisons that connection before the
/// error reaches the caller: the transaction state is unknown from that point on,
/// and the connection refuses further work with
/// [`PgError::ConnectionPoisoned`] instead of running it inside the abandoned
/// transaction. Because the handle borrows the connection mutably, that refusal
/// becomes observable exactly when the borrow ends.
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
    /// Poison record shared with the connection that opened the transaction.
    poison: Arc<PoisonCell>,
}

impl PgAsyncTransaction<'_> {
    /// Commits the transaction, persisting all its work.
    ///
    /// A commit the driver refuses poisons the connection and returns the driver
    /// failure: the transaction state is unknown from that point on, so the
    /// connection refuses every further statement with
    /// [`PgError::ConnectionPoisoned`] instead of running the work of the next
    /// caller inside it.
    pub async fn commit(self) -> Result<(), PgError> {
        let PgAsyncTransaction { inner, poison } = self;
        match inner.commit().await {
            Ok(()) => {
                tracing::info!("committed transaction");
                Ok(())
            }
            Err(error) => {
                let failure = PgError::from(error);
                poison.mark(control_failure("COMMIT", &failure));
                Err(failure)
            }
        }
    }

    /// Rolls the transaction back, discarding all its work.
    ///
    /// A rollback the driver refuses poisons the connection for the same reason a
    /// refused commit does: the statements of the transaction stay pending on the
    /// server, and nothing the caller could run next is outside them.
    pub async fn rollback(self) -> Result<(), PgError> {
        let PgAsyncTransaction { inner, poison } = self;
        match inner.rollback().await {
            Ok(()) => {
                tracing::warn!("rolled back transaction");
                Ok(())
            }
            Err(error) => {
                let failure = PgError::from(error);
                poison.mark(control_failure("ROLLBACK", &failure));
                Err(failure)
            }
        }
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
