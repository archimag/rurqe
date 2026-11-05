//! Asynchronous execution adapter over [`tokio_postgres::Client`].
//!
//! The adapter tracks the transaction state of the connection it drives and
//! poisons it whenever it cannot prove that the connection is outside a
//! transaction: a `COMMIT` or a `ROLLBACK` the driver refused, a `BEGIN` that
//! failed, a transaction entry whose future was dropped before the transaction
//! opened, a panicking block, a block whose future was dropped before either
//! control statement ran, or a transaction handle dropped without a terminal
//! call. A poisoned connection refuses every statement and every
//! transaction with [`PgError::ConnectionPoisoned`] instead of executing them
//! inside the abandoned transaction, which PostgreSQL would accept with a warning
//! and persist with the next `COMMIT`. The state is sticky and is never cleared:
//! the owner of the handle discards it, and the abandoned transaction closes when
//! the socket of the discarded connection closes.
//!
//! The closure-scoped settlement — the entry refusal, the `BEGIN`, the awaited
//! ending, and the recording of a refused rollback — lives in [`settle_block`],
//! so the asynchronous pool runs the same protocol instead of a second copy of
//! it.
use std::sync::Arc;

use futures_core::Stream;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery, IsolationLevel};

use super::async_scope::{execute_control, PgAsyncScope, ScopeFuture};
use super::poison::{control_failure, BlockExitGuard, PoisonCell, ABANDONED_BLOCK};
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
/// Closes the transaction block a closure-scoped form opened on `client`.
///
/// The record decides the ending. A clean record commits, and a `COMMIT` the
/// driver refuses is recorded on it, so the connection refuses further work
/// instead of running it inside a transaction whose state is unknown. A
/// poisoned record must not commit at all: the statements the record sealed off
/// are exactly the ones that could not be undone, so the block is rolled back
/// and the call reports the poisoned connection instead of the value the
/// closure returned. The rollback is best effort, because the connection is
/// discarded either way, and a refusal it meets is recorded on the same record,
/// whose first reason stays the one that explains the state.
pub(crate) async fn close_block(
    client: &tokio_postgres::Client,
    poison: &PoisonCell,
) -> Result<(), PgError> {
    if !poison.is_poisoned() {
        return match execute_control(client, String::from("COMMIT")).await {
            Ok(()) => Ok(()),
            Err(failure) => {
                poison.mark(control_failure("COMMIT", &failure));
                Err(failure)
            }
        };
    }

    let reason = poison
        .poison_reason()
        .unwrap_or_else(|| String::from(super::poison::NO_RECORDED_REASON));
    tracing::error!(
        reason = %reason,
        "the connection is poisoned, so the block is rolled back instead of committed"
    );

    match execute_control(client, String::from("ROLLBACK")).await {
        Ok(()) => tracing::warn!("rolled back the block of a poisoned connection"),
        Err(failure) => {
            poison.mark(control_failure("ROLLBACK", &failure));
            tracing::error!(
                failure = %failure,
                "rolling back the block of a poisoned connection failed; the connection \
                 must be discarded"
            );
        }
    }

    Err(poison.refusal())
}

/// Runs `f` inside the transaction `begin` opens on `client`, settling the
/// block and the poison record once the closure returns.
///
/// The function is the one copy of the closure-scoped protocol both
/// asynchronous adapters run, so the connection form and the pooled form
/// cannot disagree about it: the entry refuses a connection the record already
/// poisons, a `BEGIN` the driver refuses is recorded on that record, the
/// closure runs against a [`PgAsyncScope`] borrowing it, and the ending —
/// `COMMIT`, a rollback of a poisoned block, or the rollback of a block that
/// returned `Err` — is awaited before the result reaches the caller.
///
/// The error of the block is the one that reaches the caller. A `ROLLBACK` the
/// driver refuses is recorded on the record together with the driver failure
/// behind it, because the block reports its own error instead: a caller
/// inspects that failure through the refusal a later statement receives, and
/// the adapter that owns the connection destroys it. A `COMMIT` the driver
/// refuses returns its own failure, and a block whose record is poisoned is
/// rolled back and reported as the poisoned connection instead of being
/// committed.
///
/// The adapter on top decides what an unproven ending costs: a connection
/// refuses its later work, while a pooled checkout is destroyed instead of
/// being returned to the pool.
pub(crate) async fn settle_block<T, E, F>(
    client: &tokio_postgres::Client,
    poison: &Arc<PoisonCell>,
    begin: String,
    f: F,
) -> Result<T, E>
where
    E: From<PgError>,
    F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
{
    if poison.is_poisoned() {
        return Err(E::from(poison.refusal()));
    }

    let mut guard = BlockExitGuard::new(Arc::clone(poison));
    if let Err(failure) = execute_control(client, begin).await {
        poison.mark(control_failure("BEGIN", &failure));
        return Err(E::from(failure));
    }

    match f(PgAsyncScope::new(client, Arc::clone(poison))).await {
        Ok(value) => {
            // The block closed its transaction with an awaited statement, or
            // the poisoned record turned that commit into a rollback, which
            // `close_block` reports as the refusal of the connection.
            close_block(client, poison).await?;
            guard.mark_completed();
            tracing::info!("committed transaction");
            Ok(value)
        }
        Err(error) => {
            match execute_control(client, String::from("ROLLBACK")).await {
                Ok(()) => {
                    guard.mark_completed();
                    tracing::warn!("rolled back transaction");
                }
                Err(failure) => {
                    let reason = control_failure("ROLLBACK", &failure);
                    tracing::error!(
                        failure = %failure,
                        "rolling back the transaction failed; the connection is \
                         poisoned and must be discarded"
                    );
                    // The block reports its own error instead of this one, so
                    // the driver failure is recorded on the cell and handed
                    // back by the refusal a later statement receives.
                    poison.mark_with_cause(reason, failure);
                }
            }
            Err(error)
        }
    }
}

/// Returns the session of a checked-out asynchronous client to a provably idle
/// state.
///
/// A connection a pool hands back carries the session state of the block that
/// drove it last: a transaction a caller opened through the raw driver pool, a
/// `BEGIN` whose `COMMIT` nobody issued, or a block whose rollback the driver
/// refused. The reset is one round trip of the simple protocol carrying a single
/// `ROLLBACK`, the one transaction-control statement PostgreSQL answers in every
/// session state: a session that still holds a transaction has it ended —
/// including the aborted one, where the server rejects a `BEGIN` with `25P02`
/// and would abort the rest of a multi-statement reset — and a session that is
/// already idle answers with the warning `25P01` instead of an error. The idle
/// state is therefore guaranteed without classifying the state the session
/// arrived in.
///
/// A reset the driver refuses means the connection could not be brought back to
/// a known state, so the failure is reported in the
/// [`PgError::AbandonedTransaction`] variant instead of the session being handed
/// to the next caller: the driver failure travels whole, so an operator reaches
/// the cause it reports through [`source`](std::error::Error::source).
///
/// The callers are the pools, because only a checkout can still close the state
/// a previous block left behind: the bookkeeping of an adapter dies with the
/// adapter, while the session of the server outlives it.
#[cfg_attr(not(all(feature = "deadpool-postgres", feature = "tokio-postgres")), allow(dead_code))]
pub(crate) async fn ensure_idle_async(client: &tokio_postgres::Client) -> Result<(), PgError> {
    client
        .batch_execute("ROLLBACK")
        .await
        .map_err(|failure| PgError::AbandonedTransaction { failure })
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
    /// driver refused, an entry that ended before its transaction opened, or a
    /// block that ended without one. Every further statement
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
        if self.poison.is_poisoned() {
            Err(self.poison.refusal())
        } else {
            Ok(())
        }
    }

    /// Begins a transaction over the connection.
    ///
    /// The returned [`PgAsyncTransaction`] issues `COMMIT` only when
    /// [`commit`](PgAsyncTransaction::commit) is called. Dropping the handle
    /// without committing requests a rollback through the driver's `Drop`, which
    /// is a fire-and-forget statement the driver neither awaits nor reports, so a
    /// cancelled or panicking block is rolled back only as far as the driver
    /// manages. The dropped handle records the abandoned transaction in the poison
    /// record of the connection, so every further statement and every further
    /// transaction on it is refused with [`PgError::ConnectionPoisoned`] instead
    /// of joining the abandoned transaction.
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
    ///
    /// The entry is guarded like the transaction it opens. A `BEGIN` the driver
    /// refuses records the failure on the poison cell of the connection and returns
    /// the driver error, so the session is never handed to a later statement while
    /// the server may still hold a transaction. A caller that drops the future of
    /// this method before the transaction opened poisons the connection in the same
    /// way: the adapter cannot prove that the `BEGIN` did not reach the server, so
    /// it refuses the session instead of assuming it was left outside a
    /// transaction.
    pub async fn transaction(&mut self) -> Result<PgAsyncTransaction<'_>, PgError> {
        self.refuse_if_poisoned()?;
        let poison = Arc::clone(&self.poison);
        let mut guard = BlockExitGuard::new(Arc::clone(&poison));
        tracing::info!("beginning transaction");
        let inner = match self.client.transaction().await {
            Ok(inner) => inner,
            Err(error) => {
                let failure = PgError::from(error);
                poison.mark(control_failure("BEGIN", &failure));
                return Err(failure);
            }
        };
        guard.mark_completed();
        Ok(PgAsyncTransaction {
            inner: Some(inner),
            poison,
        })
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
    /// A nested block shares the same record: a savepoint rollback the driver
    /// refuses is recorded on the cell of the connection before the error of the
    /// block reaches the caller, so a connection whose savepoint could not be
    /// undone refuses further work as well.
    ///
    /// The refusal reaches the commit as well: a block whose closure returns
    /// `Ok` while the record is poisoned is not committed. The adapter rolls its
    /// statements back and reports the poisoned-connection error instead of the
    /// value the closure returned, so the statements whose undo failed are never
    /// persisted.
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
        let mut begin = String::from("BEGIN ISOLATION LEVEL ");
        begin.push_str(isolation.as_str());
        settle_block(&self.client, &self.poison, begin, f).await
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
///
/// A handle that ends without one of its terminal calls poisons the connection
/// instead of leaving an abandoned transaction behind: the rollback the driver
/// requests from its own `Drop` is a fire-and-forget statement the adapter
/// neither awaits nor observes, so the transaction may stay open on the server,
/// and the connection refuses further work rather than running the next caller
/// inside it. A terminal call that is cancelled is the same ending one step
/// later: the handle has already handed its transaction to the awaited statement
/// by then, so the call guards that statement itself and records the poison when
/// its future is dropped before the statement resolves.
pub struct PgAsyncTransaction<'a> {
    /// The open transaction, taken by the terminal call that ends it.
    ///
    /// The field is optional so that the terminal calls can move the transaction
    /// out and await its end: a `Drop` implementation cannot move a field out of
    /// the value it drops, and a transaction still present when the handle drops
    /// is exactly the state that poisons the connection.
    inner: Option<tokio_postgres::Transaction<'a>>,
    /// Poison record shared with the connection that opened the transaction.
    poison: Arc<PoisonCell>,
}

impl<'a> PgAsyncTransaction<'a> {
    /// Commits the transaction, persisting all its work.
    ///
    /// The terminal call takes the transaction out of the handle before it awaits
    /// the commit, so the handle ends with nothing pending: the transaction it
    /// carried was closed by the awaited statement. The statement is guarded
    /// while it is in flight, so a future dropped before it resolves poisons the
    /// connection instead of leaving the transaction unproven — nothing is left
    /// for the handle's own `Drop` to request at that point. A commit the driver
    /// refuses poisons the connection and returns the driver failure: the
    /// transaction state is unknown from that point on, so the connection refuses
    /// every further statement with [`PgError::ConnectionPoisoned`] instead of
    /// running the work of the next caller inside it.
    pub async fn commit(mut self) -> Result<(), PgError> {
        let Some(inner) = self.inner.take() else {
            return Err(consumed_refusal());
        };
        // The handle carries no transaction from here on, so its own `Drop` can
        // prove nothing about it: the guard records the poison when the future
        // of this call is dropped before the statement resolved.
        let mut guard = BlockExitGuard::new(Arc::clone(&self.poison));

        let outcome = inner.commit().await;
        // The statement resolved, so the ending it produced is the one recorded
        // below: the guard has nothing left to report.
        guard.mark_completed();

        match outcome {
            Ok(()) => {
                tracing::info!("committed transaction");
                Ok(())
            }
            Err(error) => {
                let failure = PgError::from(error);
                self.poison.mark(control_failure("COMMIT", &failure));
                Err(failure)
            }
        }
    }

    /// Rolls the transaction back, discarding all its work.
    ///
    /// The terminal call takes the transaction out of the handle exactly as
    /// [`commit`](PgAsyncTransaction::commit) does, and it arms the same guard for
    /// the statement it awaits: a future dropped before the rollback resolves
    /// poisons the connection, because the statements of the transaction stay
    /// pending on the server and nothing the caller could run next is outside
    /// them. A rollback the driver refuses poisons the connection for the same
    /// reason a refused commit does.
    pub async fn rollback(mut self) -> Result<(), PgError> {
        let Some(inner) = self.inner.take() else {
            return Err(consumed_refusal());
        };
        // The handle carries no transaction from here on, so its own `Drop` can
        // prove nothing about it: the guard records the poison when the future
        // of this call is dropped before the statement resolved.
        let mut guard = BlockExitGuard::new(Arc::clone(&self.poison));

        let outcome = inner.rollback().await;
        // The statement resolved, so the ending it produced is the one recorded
        // below: the guard has nothing left to report.
        guard.mark_completed();

        match outcome {
            Ok(()) => {
                tracing::warn!("rolled back transaction");
                Ok(())
            }
            Err(error) => {
                let failure = PgError::from(error);
                self.poison.mark(control_failure("ROLLBACK", &failure));
                Err(failure)
            }
        }
    }
}

impl Drop for PgAsyncTransaction<'_> {
    /// Records the abandoned transaction when the handle ends without a terminal
    /// call.
    ///
    /// The terminal calls take the transaction out before they await its end, so a
    /// transaction this handle still carries here is one no statement ever closed.
    /// The rollback the driver requests from the `Drop` of its own transaction is
    /// a fire-and-forget statement the adapter neither awaits nor observes, so the
    /// server-side transaction may stay open; the connection is poisoned instead
    /// of letting a later caller run inside it.
    /// A terminal call whose own future was dropped before its statement resolved
    /// is recorded by the guard that call arms, not here: by then this handle
    /// already carries no transaction.
    fn drop(&mut self) {
        if self.inner.is_some() {
            self.poison.mark(String::from(ABANDONED_BLOCK));
        }
    }
}

/// Describes a statement issued on a handle whose transaction a terminal call
/// already took.
///
/// The terminal calls consume the handle, so the state is not reachable through
/// the public API; the refusal keeps it diagnosable instead of unwrapping a value
/// the caller never observed.
fn consumed_refusal() -> PgError {
    PgError::ConnectionPoisoned {
        reason: String::from("the transaction handle was already consumed by a terminal call"),
        cause: None,
    }
}

/// Borrows the open transaction of a handle, refusing a consumed one.
fn live_transaction<'a, 'b>(
    inner: &'a Option<tokio_postgres::Transaction<'b>>,
) -> Result<&'a tokio_postgres::Transaction<'b>, PgError> {
    inner.as_ref().ok_or_else(consumed_refusal)
}

impl AsyncQueryBackend for PgAsyncTransaction<'_> {
    type Error = PgError;

    async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        let inner = live_transaction(&self.inner)?;
        fetch_raw_async(inner, query).await
    }

    async fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, PgError> {
        let inner = live_transaction(&self.inner)?;
        fetch_raw_limited_async(inner, query, limit).await
    }

    async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        let inner = live_transaction(&self.inner)?;
        execute_raw_async(inner, query).await
    }
}
