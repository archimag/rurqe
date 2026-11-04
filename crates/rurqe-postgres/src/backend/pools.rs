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
//!
//! A block is still one checkout, so the transaction state the adapter guards
//! is the state of a session that outlives it. `PgPool::get` therefore hands the
//! connection out as a guard, and the guard repairs the session when the
//! checkout ends — but only when the pool cannot prove the session to be idle
//! already: a statement and a transaction block the pool drives close the work
//! they open, so a checkout of its own costs no round trip, while a session a
//! raw handle left inside a transaction, or one whose block ended without
//! closing it, is repaired on the way out instead of enclosing the next block.
//!
//! The asynchronous `PgAsyncPool` exposes the same closure-scoped form: its
//! `transaction` and `transaction_with_isolation` keep one pooled connection for
//! the whole block, let the block nest through savepoints, and close it with an
//! awaited `COMMIT` or `ROLLBACK`. The checkout is never returned to the pool
//! with its transaction open: a block that is cancelled, that unwinds, or whose
//! control statement the driver refuses destroys its own connection, so the pool
//! opens a fresh one for the next caller instead of recycling a session that
//! still holds an abandoned transaction. A block whose connection a refused
//! savepoint rollback poisoned is never committed either: it is rolled back and
//! reported as the poisoned-connection failure, and its checkout is destroyed
//! the same way.
//!
//! Every checkout the pool mediates for a closure-scoped block is proven idle
//! before that block opens: one `ROLLBACK` through the simple query protocol is
//! issued before `BEGIN`, which PostgreSQL answers in every session state. A
//! session that reached the pool with a transaction still open — a checkout a
//! caller drove through the raw pool, or one a transaction-control statement
//! left open through a statement path — has that transaction ended before the
//! block opens, so the block can neither join the abandoned transaction nor
//! persist its work with its own `COMMIT`. A proof the driver refuses records
//! its reason on the shared poison record, reports
//! `PgError::AbandonedTransaction`, and destroys the checkout like every other
//! unclean ending, so a session the pool cannot prove idle never reaches
//! another caller. The synchronous pools close the same hazard from the other
//! side, by repairing the session when a checkout ends.
//!
//! A single statement driven through either pool — `fetch_raw`,
//! `fetch_raw_limited`, or `execute_raw` — runs in autocommit. Issuing a
//! transaction-control statement (`BEGIN`, `SAVEPOINT`, `COMMIT`, `ROLLBACK`)
//! through that path breaks the contract rather than being refused: the session
//! returns to the pool with a transaction still open, which the synchronous
//! pool hands to the next checkout as it is, while every checkout of the
//! asynchronous pool — a statement checkout as well as a block — is proven idle
//! before it runs and therefore ends the abandoned transaction instead of
//! joining it. A statement checkout whose future is dropped before its statement
//! settled is destroyed rather than recycled, because a `Drop` cannot await the
//! proof that would settle it. Work that spans several statements belongs in
//! `PgPool::transaction`, `PgPool::transaction_with_isolation`,
//! `PgAsyncPool::transaction`, or `PgAsyncPool::transaction_with_isolation`,
//! which keep one connection checked out for the whole block and settle the
//! transaction state it ends in.
//!
//! Each pool is gated on the driver it pools, so the feature that enables a pool
//! carries its driver and the module of that pool is compiled on the pair.

#[cfg(all(feature = "deadpool-postgres", feature = "tokio-postgres"))]
mod checkout;

#[cfg(all(feature = "deadpool-postgres", feature = "tokio-postgres"))]
mod deadpool_impl {
    use std::sync::Arc;

    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{AsyncQueryBackend, BuiltQuery, IsolationLevel};

    use crate::backend::async_conn::{
        close_block, execute_raw_async, fetch_raw_async, fetch_raw_limited_async,
    };
    use crate::backend::async_scope::{execute_control, PgAsyncScope, ScopeFuture};
    use crate::backend::poison::{control_failure, PoisonCell};
    use crate::backend::types::PgError;

    use super::checkout::Checkout;

    /// A `deadpool-postgres` pool executing RURQE queries asynchronously.
    ///
    /// The pool implements the asynchronous
    /// [`AsyncQueryBackend`](rurqe_core::query::AsyncQueryBackend) contract, so
    /// the `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`AsyncQueryBackendExt`](rurqe_core::query::AsyncQueryBackendExt) become
    /// available directly on the pool: every call checks a connection out,
    /// awaits the statement, and returns the connection to the pool.
    ///
    /// A multi-statement block goes through
    /// [`transaction`](PgAsyncPool::transaction), which keeps one connection
    /// checked out for the whole block and commits it as a whole.
    ///
    /// A single statement is the other path: `fetch_raw`, `fetch_raw_limited`,
    /// and `execute_raw` check a connection out, prove its session idle, await
    /// one statement in autocommit, and return the checkout to the pool once
    /// the statement settled. The proof keeps a statement out of a transaction
    /// an earlier checkout left open, and a checkout whose future is dropped
    /// before its statement settled is destroyed instead of recycled. A
    /// transaction-control statement (`BEGIN`, `SAVEPOINT`, `COMMIT`,
    /// `ROLLBACK`) issued through that path breaks the contract rather than
    /// being refused: the session returns to the pool inside a transaction the
    /// proof of the next checkout ends. Every statement that opens, closes, or
    /// nests a transaction therefore belongs in
    /// [`transaction`](PgAsyncPool::transaction) or
    /// [`transaction_with_isolation`](PgAsyncPool::transaction_with_isolation).
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

        /// Checks a connection out of the pool and proves its session idle.
        ///
        /// One `ROLLBACK` through the simple query protocol — the statement
        /// PostgreSQL answers in every session state — ends whatever transaction
        /// the session still held, so neither a block nor a statement joins it. A
        /// refused proof records its reason on the record of the checkout and is
        /// returned, so the guard destroys the session instead of handing it out.
        async fn acquire(
            &self,
            guard: fn(deadpool_postgres::Object, Arc<PoisonCell>) -> Checkout,
        ) -> Result<Checkout, PgError> {
            let object = self.pool.get().await.map_err(PgError::from)?;
            let checkout = guard(object, Arc::new(PoisonCell::new()));
            let client = checkout.session()?;
            if let Err(failure) = crate::backend::async_conn::ensure_idle_async(client).await {
                checkout.poison().mark(control_failure("ROLLBACK", &failure));
                return Err(failure);
            }
            Ok(checkout)
        }
        /// Runs `f` inside a transaction on a pooled connection, committing on
        /// `Ok` and rolling back on `Err`.
        ///
        /// The connection stays checked out for the whole block, so every
        /// statement of `f` runs on one physical connection and the block is
        /// atomic exactly as it is on an owned connection: the outermost call
        /// runs between `BEGIN` and `COMMIT`, a call made while a transaction is
        /// already open nests inside a `SAVEPOINT` through
        /// [`PgAsyncScope::transaction`], and a block returning `Err` has its
        /// statements rolled back before the error reaches the caller. The pool
        /// itself is shared, so several transactions can run concurrently: every
        /// call checks its own connection out, and the checkout is settled on
        /// every exit path, because the guard that owns it is dropped however the
        /// block ends.
        ///
        /// Every checkout the method mediates is proven idle before the block
        /// opens: one `ROLLBACK` through the simple query protocol is issued
        /// before `BEGIN`, which PostgreSQL answers in every session state. A
        /// session that reached the pool with a transaction still open — a
        /// checkout driven through [`pool`](PgAsyncPool::pool), or one a
        /// transaction-control statement left open through a statement path —
        /// has that transaction ended before the block opens, so the block can
        /// neither join the abandoned transaction nor persist its work. A proof
        /// the driver refuses destroys the checkout and surfaces as
        /// [`PgError::AbandonedTransaction`] instead of opening the block on a
        /// session the pool cannot vouch for.
        ///
        /// ```text
        /// pool
        ///     .transaction(|scope| {
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
        /// The closure erases its own future into a boxed [`Send`] one, exactly
        /// as the closure of
        /// [`crate::PgAsyncConnection::transaction_with_isolation`] does, because
        /// an asynchronous terminal call returns an opaque `impl Future` that a
        /// closure return type cannot name on stable Rust. `E` is the error of
        /// `f` and has to absorb the driver error, which the dialect error alias
        /// over [`PgError`] does, so a terminal call propagates with `?`
        /// unchanged inside the block. The bound on `E` is the one the closure
        /// form of [`crate::PgAsyncConnection::transaction_with_isolation`]
        /// declares, so a block written against a connection runs on the pool
        /// unchanged.
        ///
        /// A checkout failure surfaces as [`PgError::Pool`]; a failure raised by
        /// the idle proof, by `BEGIN`, by `f`, or by `COMMIT` surfaces as
        /// [`PgError`]. A savepoint rollback the driver refuses inside the block
        /// is recorded on the poison record of the connection, which is the
        /// record the guard reads, so the
        /// record gates the commit: a block that returns `Ok` while its record is
        /// poisoned is rolled back and reported as [`PgError::ConnectionPoisoned`]
        /// instead of being committed.
        ///
        /// # Abandoned blocks
        ///
        /// A block that is cancelled or unwound never reaches the end of this
        /// method, so neither `COMMIT` nor `ROLLBACK` is awaited for it: the
        /// checkout guard is dropped on the way out, and it destroys the
        /// connection instead of returning it to the pool. A refused `BEGIN`,
        /// `COMMIT`, or `ROLLBACK` destroys the connection the same way, so a
        /// session that still holds the abandoned transaction never reaches
        /// another caller: dropping the client closes the socket, and the server
        /// rolls the transaction back when the session ends.
        ///
        /// The synchronous pools differ here: their guard performs the rollback
        /// in its `Drop` before the connection returns to the pool.
        ///
        /// A block that has to survive an abandoned future therefore belongs in
        /// a task that finishes or reports its own cancellation; the pool itself
        /// stays usable, because the connection it lost is replaced.
        pub async fn transaction<T, E, F>(&self, f: F) -> Result<T, E>
        where
            E: From<PgError>,
            F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
        {
            let checkout = self.acquire(Checkout::new).await.map_err(E::from)?;
            run_transaction(checkout, String::from("BEGIN"), f).await
        }

        /// Runs `f` inside a transaction at the requested `isolation` level on a
        /// pooled connection, committing on `Ok` and rolling back on `Err`.
        ///
        /// The helper mirrors [`PgAsyncPool::transaction`] and differs only in
        /// the statement that opens the outermost block: the block runs between
        /// `BEGIN ISOLATION LEVEL <level>` and `COMMIT`, with the level text
        /// coming from [`IsolationLevel::as_str`], so every statement of `f`
        /// observes the isolation the caller asked for. A call made while a
        /// transaction is already open nests inside a `SAVEPOINT` and the
        /// requested level is ignored for that nested block, because SQL defines
        /// no per-savepoint isolation and the enclosing transaction keeps the
        /// level it was opened with.
        ///
        /// The idle proof that precedes the block, the error mapping, and the
        /// abandoned-block failure mode are the ones documented on
        /// [`PgAsyncPool::transaction`]: the checkout is proven idle before
        /// `BEGIN` is dispatched, a checkout failure surfaces as
        /// [`PgError::Pool`], a failure raised by the proof, by `BEGIN`, by `f`,
        /// or by `COMMIT` surfaces as [`PgError`], and a cancelled, unwound, or
        /// control-failed block destroys its connection instead of returning it
        /// to the pool.
        pub async fn transaction_with_isolation<T, E, F>(
            &self,
            isolation: IsolationLevel,
            f: F,
        ) -> Result<T, E>
        where
            E: From<PgError>,
            F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
        {
            let checkout = self.acquire(Checkout::new).await.map_err(E::from)?;

            let mut begin = String::from("BEGIN ISOLATION LEVEL ");
            begin.push_str(isolation.as_str());
            run_transaction(checkout, begin, f).await
        }
    }

    /// Runs `f` inside the transaction `begin` opens on `checkout` and settles
    /// the guard once the closure returns.
    /// The commit of a block that returned `Ok` and the rollback of a block that
    /// returned `Err` are both awaited, so the transaction is closed on the
    /// server before the result reaches the caller. The checkout is then either
    /// returned to the pool or destroyed:
    ///
    /// * a block that closed its transaction with an awaited statement and left
    ///   the shared poison record clean returns its connection to the pool;
    /// * a `BEGIN`, `COMMIT`, or `ROLLBACK` the driver refused, and a block whose
    ///   savepoint could not be undone, destroy the connection, because the
    ///   session still holds work the adapter cannot roll back;
    /// * a block whose closure returns `Ok` after a refused savepoint rollback
    ///   poisoned the record is not committed: it is rolled back and the call
    ///   reports the poisoned-connection failure instead of the value of the
    ///   block, and the connection is destroyed like the other unclean endings;
    /// * a block that unwound, or whose future was dropped, never reaches the end
    ///   of this method at all, and the guard destroys the connection on the way
    ///   out.
    ///
    /// A `ROLLBACK` the driver refuses supersedes the error of the block: the
    /// refused statement is recorded on the connection, and the caller receives
    /// [`PgError::ConnectionPoisoned`] carrying it through
    /// [`source`](std::error::Error::source), because the transaction state is
    /// unknown from that point on.
    ///
    /// The checkout is proven idle before the block opens. One `ROLLBACK`
    /// through the simple query protocol — the statement PostgreSQL answers in
    /// every session state — ends whatever transaction the session still held
    /// before the opening `BEGIN` is dispatched, so the block can neither join
    /// an abandoned transaction nor persist its work with its own `COMMIT`. A
    /// proof the driver refuses records its reason on the shared poison record,
    /// returns [`PgError::AbandonedTransaction`], and destroys the checkout like
    /// every other unclean ending.
    ///
    /// The block shares the poison record of the checkout instead of receiving a
    /// record of its own: a savepoint rollback the driver refuses is recorded on
    /// the cell the guard reads, which turns that failure into a destroyed
    /// connection rather than a diagnostic nobody acts on.
    async fn run_transaction<T, E, F>(
        mut checkout: Checkout,
        begin: String,
        f: F,
    ) -> Result<T, E>
    where
        E: From<PgError>,
        F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
    {
        let poison = Arc::clone(checkout.poison());
        let client = checkout.session().map_err(E::from)?;
        if let Err(failure) = execute_control(client, begin).await {
            poison.mark(control_failure("BEGIN", &failure));
            return Err(E::from(failure));
        }

        match f(PgAsyncScope::new(client, Arc::clone(&poison))).await {
            Ok(value) => {
                let closed = close_block(client, &poison).await;
                match closed {
                    Ok(()) => {
                        tracing::info!("committed transaction");
                        checkout.recycle();
                        Ok(value)
                    }
                    Err(failure) => Err(E::from(failure)),
                }
            }
            Err(error) => match execute_control(client, String::from("ROLLBACK")).await {
                Ok(()) => {
                    tracing::warn!("rolled back transaction");
                    checkout.recycle();
                    Err(error)
                }
                Err(failure) => {
                    let reason = control_failure("ROLLBACK", &failure);
                    tracing::error!(
                        failure = %failure,
                        "rolling back the transaction failed; the connection is destroyed \
                         and the failure of the block is superseded"
                    );
                    // The refusal supersedes the error of the block, so the
                    // driver failure is recorded on the cell and handed back by
                    // the refusal instead of surviving in the trace alone.
                    poison.mark_with_cause(reason, failure);
                    Err(E::from(poison.refusal()))
                }
            },
        }
    }

    impl AsyncQueryBackend for PgAsyncPool {
        type Error = PgError;

        /// Reads every row of `query` on a checkout whose session is idle.
        ///
        /// The proof that precedes the statement keeps it out of a transaction
        /// an earlier checkout left open, and the checkout returns to the pool
        /// once the statement settled, on a refusal as well. A future dropped
        /// before that leaves the checkout unsettled, so the guard destroys the
        /// connection instead of recycling it.
        async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let mut checkout = self.acquire(Checkout::statement).await?;
            let client = checkout.session()?;
            let rows = fetch_raw_async(client, query).await;
            checkout.recycle();
            rows
        }

        /// Reads at most `limit` rows from a checkout whose session is idle.
        ///
        /// The limit reaches the row stream of the underlying
        /// `tokio_postgres` client, so a single-row lookup stops after the
        /// second row instead of materialising the whole result set. The
        /// checkout is proven idle and recycled like every other statement path
        /// of the pool.
        async fn fetch_raw_limited(
            &self,
            query: &BuiltQuery,
            limit: Option<usize>,
        ) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let mut checkout = self.acquire(Checkout::statement).await?;
            let client = checkout.session()?;
            let rows = fetch_raw_limited_async(client, query, limit).await;
            checkout.recycle();
            rows
        }

        /// Executes `query` on a checkout whose session is idle and reports the
        /// number of affected rows.
        ///
        /// The proof that precedes the statement, the recycling of the settled
        /// checkout, and the destruction of an unsettled one are the ones
        /// documented on the `fetch_raw` path of the pool.
        async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            let mut checkout = self.acquire(Checkout::statement).await?;
            let client = checkout.session()?;
            let affected = execute_raw_async(client, query).await;
            checkout.recycle();
            affected
        }
    }
}

#[cfg(all(feature = "deadpool-postgres", feature = "tokio-postgres"))]
pub use deadpool_impl::PgAsyncPool;

#[cfg(all(feature = "r2d2", feature = "postgres"))]
mod r2d2_impl;

#[cfg(all(feature = "r2d2", feature = "postgres"))]
pub use r2d2_impl::{PgPool, PgPoolError, PgPooledConnection};
