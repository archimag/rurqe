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
//! is the state of a session that outlives it. `PgPool::get` therefore returns
//! every checked-out connection to an idle session before handing it out,
//! which is what makes the guarantee hold on the pooled surface as well: a
//! connection whose session still holds a transaction — one opened through the
//! raw handle, or one whose block ended without closing it — is repaired at
//! the next checkout instead of running the next block inside the abandoned
//! transaction.
//! The asynchronous `PgAsyncPool` exposes the same closure-scoped form: its
//! `transaction` and `transaction_with_isolation` keep one pooled connection for
//! the whole block, let the block nest through savepoints, and close it with an
//! awaited `COMMIT` or `ROLLBACK`. The checkout is never returned to the pool
//! with its transaction open: a block that is cancelled, that unwinds, or whose
//! control statement the driver refuses destroys its own connection, so the pool
//! opens a fresh one for the next caller instead of recycling a session that
//! still holds an abandoned transaction.
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

    use crate::backend::async_conn::{execute_raw_async, fetch_raw_async, fetch_raw_limited_async};
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
        /// `BEGIN`, by `f`, or by `COMMIT` surfaces as [`PgError`]. A savepoint
        /// rollback the driver refuses inside the block is recorded on the poison
        /// record of the connection, which is the record the guard reads, so the
        /// connection is destroyed instead of being returned to the pool.
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
            let object = self.pool.get().await.map_err(PgError::from)?;
            run_transaction(object, String::from("BEGIN"), f).await
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
        /// The error mapping and the abandoned-block failure mode are the ones
        /// documented on [`PgAsyncPool::transaction`]: a checkout failure
        /// surfaces as [`PgError::Pool`], a failure raised by `BEGIN`, by `f`,
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
            let object = self.pool.get().await.map_err(PgError::from)?;

            let mut begin = String::from("BEGIN ISOLATION LEVEL ");
            begin.push_str(isolation.as_str());
            run_transaction(object, begin, f).await
        }
    }

    /// Runs `f` inside the transaction `begin` opens on `object` and settles the
    /// checkout once the closure returns.
    ///
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
    /// * a block that unwound, or whose future was dropped, never reaches the end
    ///   of this method at all, and the guard destroys the connection on the way
    ///   out.
    ///
    /// A `ROLLBACK` the driver refuses supersedes the error of the block: the
    /// refused statement is traced, and the caller receives
    /// [`PgError::ConnectionPoisoned`] carrying it, because the transaction
    /// state is unknown from that point on.
    ///
    /// The block shares the poison record of the checkout instead of receiving a
    /// record of its own: a savepoint rollback the driver refuses is recorded on
    /// the cell the guard reads, which turns that failure into a destroyed
    /// connection rather than a diagnostic nobody acts on.
    async fn run_transaction<T, E, F>(
        object: deadpool_postgres::Object,
        begin: String,
        f: F,
    ) -> Result<T, E>
    where
        E: From<PgError>,
        F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
    {
        let poison = Arc::new(PoisonCell::new());
        let mut checkout = Checkout::new(object, Arc::clone(&poison));

        let Some(client) = checkout.client() else {
            return Err(E::from(PgError::ConnectionPoisoned {
                reason: String::from("the checked-out connection is already destroyed"),
            }));
        };

        if let Err(failure) = execute_control(client, begin).await {
            poison.mark(control_failure("BEGIN", &failure));
            return Err(E::from(failure));
        }

        match f(PgAsyncScope::new(client, Arc::clone(&poison))).await {
            Ok(value) => match execute_control(client, String::from("COMMIT")).await {
                Ok(()) => {
                    tracing::info!("committed transaction");
                    checkout.recycle();
                    Ok(value)
                }
                Err(failure) => {
                    poison.mark(control_failure("COMMIT", &failure));
                    Err(E::from(failure))
                }
            },
            Err(error) => match execute_control(client, String::from("ROLLBACK")).await {
                Ok(()) => {
                    tracing::warn!("rolled back transaction");
                    checkout.recycle();
                    Err(error)
                }
                Err(failure) => {
                    poison.mark(control_failure("ROLLBACK", &failure));
                    tracing::error!(
                        failure = %failure,
                        "rolling back the transaction failed; the connection is destroyed \
                         and the failure of the block is superseded"
                    );
                    let reason = poison
                        .poison_reason()
                        .unwrap_or_else(|| String::from("the connection recorded no reason"));
                    Err(E::from(PgError::ConnectionPoisoned { reason }))
                }
            },
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

#[cfg(all(feature = "deadpool-postgres", feature = "tokio-postgres"))]
pub use deadpool_impl::PgAsyncPool;

#[cfg(all(feature = "r2d2", feature = "postgres"))]
mod r2d2_impl {
    use std::error::Error as StdError;
    use std::fmt;

    use r2d2::ManageConnection;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, IsolationLevel, QueryBackend};

    use crate::backend::sync_conn::{
        ensure_idle, execute_raw_sync, fetch_raw_limited_sync, fetch_raw_sync, PgConnection,
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

    // `r2d2::Pool` is an `Arc`-backed handle that clones without requiring the
    // manager to be `Clone`, so the implementation is written out instead of
    // derived: a derive would add an `M: Clone` bound to the handle.
    impl<M: ManageConnection> Clone for PgPool<M> {
        fn clone(&self) -> Self {
            Self {
                pool: self.pool.clone(),
            }
        }
    }

    impl<M: ManageConnection<Connection = postgres::Client>> PgPool<M> {
        /// Wraps an existing `r2d2` pool managing `postgres` connections.
        pub fn new(pool: r2d2::Pool<M>) -> Self {
            Self { pool }
        }

        /// Returns a shared reference to the wrapped pool.
        pub fn pool(&self) -> &r2d2::Pool<M> {
            &self.pool
        }

        /// Checks a connection out of the pool.
        /// Checks a connection out of the pool and returns it to an idle session.
        ///
        /// A checkout failure surfaces as [`PgPoolError::Pool`], so the whole
        /// pool API speaks one error vocabulary: a checkout failure and a
        /// statement failure both propagate with `?` into a function returning
        /// [`PgPoolError`].
        ///
        /// The session of a recycled connection may still hold a transaction a
        /// previous block left open, because `r2d2` validates that a connection
        /// is alive rather than that it is outside a transaction, so every
        /// checkout returns the session to an idle state before handing the
        /// connection out. The reset is one round trip of the simple protocol
        /// and leaves no trace on a session that was already idle; it is what
        /// carries the transaction guarantee across checkouts, because the
        /// bookkeeping of a single adapter dies with the adapter while the
        /// session of the reused connection does not.
        ///
        /// A reset the driver refuses means the session could not be brought
        /// back to a known state, so the checkout itself fails with
        /// [`PgPoolError::Query`] carrying [`PgError::AbandonedTransaction`]
        /// instead of handing the connection to a block that would run inside
        /// the abandoned transaction. The refused connection is dropped rather
        /// than returned to the caller, so `r2d2` discards it once the driver
        /// marks it closed.
        pub fn get(&self) -> Result<r2d2::PooledConnection<M>, PgPoolError> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            ensure_idle(&mut connection).map_err(PgPoolError::Query)?;
            Ok(connection)
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
        ///
        /// The block starts on a connection [`PgPool::get`] has returned to an
        /// idle session, so a transaction a previous checkout left open is
        /// closed before this block opens its own.
        pub fn transaction<T, F>(&self, f: F) -> Result<T, PgPoolError>
        where
            F: FnOnce(&mut PgConnection<&mut postgres::Client>) -> Result<T, PgError>,
        {
            let mut pooled = self.get()?;
            PgConnection::borrowed(&mut pooled)
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
            let mut pooled = self.get()?;
            PgConnection::borrowed(&mut pooled)
                .transaction_with_isolation(isolation, f)
                .map_err(PgPoolError::Query)
        }
    }

    /// Error raised while executing a query through an `r2d2` PostgreSQL pool.
    ///
    /// The enum is `#[non_exhaustive]`: it is the failure surface of the pool
    /// rather than a closed set, so a match written against it carries a wildcard
    /// arm, and a variant added later does not break the matches already
    /// compiled. The `Query` variant wraps [`PgError`], whose own variant set
    /// follows the features its build enables.
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum PgPoolError {
        /// The connection could not be checked out of the pool.
        Pool(r2d2::Error),
        /// The statement failed to execute, or the checkout could not return
        /// the connection to an idle session.
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

    impl From<r2d2::Error> for PgPoolError {
        fn from(error: r2d2::Error) -> Self {
            PgPoolError::Pool(error)
        }
    }

    impl From<PgError> for PgPoolError {
        fn from(error: PgError) -> Self {
            PgPoolError::Query(error)
        }
    }

    impl<M: ManageConnection<Connection = postgres::Client>> QueryBackend for PgPool<M> {
        type Error = PgPoolError;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut connection = self.get()?;
            fetch_raw_sync(&mut connection, query).map_err(PgPoolError::Query)
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
            let mut connection = self.get()?;
            fetch_raw_limited_sync(&mut connection, query, limit).map_err(PgPoolError::Query)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let mut connection = self.get()?;
            execute_raw_sync(&mut connection, query).map_err(PgPoolError::Query)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{PgError, PgPoolError};

        #[test]
        fn the_pool_error_absorbs_the_driver_error() {
            let failure = PgPoolError::from(PgError::InvalidValue {
                column_type: String::from("text"),
                reason: String::from("the value is not a text value"),
            });

            assert!(matches!(failure, PgPoolError::Query(_)));
        }
    }
}

#[cfg(all(feature = "r2d2", feature = "postgres"))]
pub use r2d2_impl::{PgPool, PgPoolError};
