use std::error::Error as StdError;
use std::fmt;
use std::ops::{Deref, DerefMut};

use r2d2::ManageConnection;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{BuiltQuery, IsolationLevel, QueryBackend, TransactionalConnection};

use crate::backend::sync_conn::{
    ensure_idle, execute_raw_sync, fetch_raw_limited_sync, fetch_raw_sync, PgConnection,
};
use crate::backend::types::PgError;

/// A connection checked out of a [`PgPool`], returned to the pool when it drops.
///
/// The synchronous driver exposes no transaction state, so the pool cannot ask
/// a session whether it is outside a transaction. The guard closes that gap
/// from the other side: it repairs the session when the checkout ends, so every
/// connection the pool hands out is in the state the pool promises, whatever
/// the previous checkout did with it.
///
/// The repair is not paid by every checkout, because the pool can prove the
/// session idle for the work it drives itself: a statement path requires its
/// statement to run in autocommit, and a transaction block ends with an
/// awaited `COMMIT` or `ROLLBACK`, so a checkout the pool drove to a clean
/// ending returns the connection without a repair. The proof of the
/// statement path is that recorded contract rather than an observation: the
/// pool grants it because a statement which runs in autocommit cannot
/// change the transaction state of the session, and a statement that does —
/// a transaction-control statement issued through the bare statement path —
/// breaks the contract, so this guard never repairs it and the next
/// checkout inherits the open transaction. A session the caller reaches
/// through the handle is the other case — the statements issued there stay
/// outside the bookkeeping of the pool — so the proof is forfeited as soon
/// as the handle is borrowed mutably, and the reset runs when the checkout
/// ends.
pub struct PgPooledConnection<M: ManageConnection<Connection = postgres::Client>> {
    /// The checked-out connection, returned to the pool when the guard drops.
    connection: r2d2::PooledConnection<M>,
    /// Whether the pool holds the idle proof of the session, granted for a
    /// statement path under the autocommit contract or observed for a
    /// transaction block that ended.
    idle: bool,
}

impl<M: ManageConnection<Connection = postgres::Client>> PgPooledConnection<M> {
    /// Wraps a checked-out connection whose session the pool has yet to prove.
    fn new(connection: r2d2::PooledConnection<M>) -> Self {
        Self {
            connection,
            idle: false,
        }
    }

    /// Records that the pool holds the idle proof of the session.
    ///
    /// A transaction block the pool drove ends with an awaited `COMMIT` or
    /// `ROLLBACK`, and a statement path requires its statement to run in
    /// autocommit, so the pool records the proof without probing a session
    /// the synchronous driver cannot ask.
    fn mark_idle(&mut self) {
        self.idle = true;
    }
}

impl<M: ManageConnection<Connection = postgres::Client>> Deref for PgPooledConnection<M> {
    type Target = postgres::Client;

    fn deref(&self) -> &postgres::Client {
        &self.connection
    }
}

impl<M: ManageConnection<Connection = postgres::Client>> DerefMut for PgPooledConnection<M> {
    /// Borrows the handle, forfeiting the proof that the session is idle.
    ///
    /// The handle reaches statements the typed builder does not model, so the
    /// pool cannot vouch for the state the session ends in, and the checkout
    /// is repaired when it ends.
    fn deref_mut(&mut self) -> &mut postgres::Client {
        self.idle = false;
        &mut self.connection
    }
}

impl<M: ManageConnection<Connection = postgres::Client>> Drop for PgPooledConnection<M> {
    /// Returns the session to an idle state when the pool cannot vouch for it.
    ///
    /// The reset is one round trip of the simple protocol carrying a
    /// single `ROLLBACK`, the one transaction-control statement
    /// PostgreSQL answers in every session state: a session that still
    /// holds a transaction has it ended — including the aborted one,
    /// where the server rejects a `BEGIN` with `25P02` — and a session
    /// that was already idle pays only the round trip of the repair,
    /// because the warning `25P01` it answers with is not an error.
    /// A reset the driver refuses leaves the session in a state the pool
    /// cannot vouch for, so the failure is logged here and the connection
    /// leaves the pool once the driver marks it closed, instead of being
    /// served to the next caller.
    fn drop(&mut self) {
        if self.idle {
            return;
        }

        if let Err(failure) = ensure_idle(&mut self.connection) {
            tracing::error!(
                failure = %failure,
                "the pooled connection could not be returned to an idle session; \
                 the pool discards it once the driver marks it closed"
            );
        }
    }
}

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
    ///
    /// The checkout costs no round trip of its own. The connection is handed
    /// out as a [`PgPooledConnection`] guard, and the guard repairs the
    /// session when the checkout ends — but only when the pool cannot prove
    /// it to be idle already, because the reset a synchronous client needs is
    /// never probed for: the pool drives every connection it hands out to a
    /// known state instead.
    ///
    /// The repair covers the session the previous checkout left behind. A
    /// connection whose session still holds a transaction — one opened
    /// through the raw handle, or one whose block ended without closing it —
    /// is reset with one round trip of the simple protocol before it returns
    /// to the pool, so it never encloses the work of the next caller. A reset
    /// the driver refuses leaves the session in a state the pool cannot vouch
    /// for: the guard reports it, and the connection leaves the pool once the
    /// driver marks it closed instead of being served again.
    ///
    /// A checkout failure surfaces as [`PgPoolError::Pool`], so the whole
    /// pool API speaks one error vocabulary: a checkout failure and a
    /// statement failure both propagate with `?` into a function returning
    /// [`PgPoolError`].
    pub fn get(&self) -> Result<PgPooledConnection<M>, PgPoolError> {
        let connection = self.pool.get().map_err(PgPoolError::Pool)?;
        Ok(PgPooledConnection::new(connection))
    }

    /// Runs `f` inside a transaction on a pooled connection, committing on
    /// `Ok` and rolling back on `Err`.
    ///
    /// The connection stays checked out for the whole block, so every
    /// statement of `f` runs on one physical connection and the block is
    /// atomic exactly as it is on an owned connection: the outermost call
    /// runs between `BEGIN` and `COMMIT`, a call made while a transaction is
    /// already open nests inside a `SAVEPOINT` through
    /// [`PgConnection::transaction`], and a block returning `Err` or panicking
    /// is rolled back. The pooled handle returns to the pool however the block
    /// ends, because the checkout guard is dropped on every exit path.
    ///
    /// A checkout failure surfaces as [`PgPoolError::Pool`]; a failure raised
    /// by `BEGIN`, by `f`, or by `COMMIT` surfaces as [`PgPoolError::Query`].
    ///
    /// The block starts on the session the checkout proves: the pool drove
    /// the previous block to its ending, and the guard repaired the session
    /// otherwise, so the adapter neither probes nor resets the connection
    /// before it opens the block. A block that ends in a state the pool cannot
    /// vouch for — a rollback the driver refused — is repaired by the guard on
    /// the way out.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, PgPoolError>
    where
        F: FnOnce(&mut PgConnection<&mut postgres::Client>) -> Result<T, PgError>,
    {
        let mut pooled = self.get()?;
        // The checkout knows the session: the pool closed it in the previous
        // block and the guard repaired it otherwise, so the adapter does not
        // probe it again before the block opens.
        let mut connection = PgConnection::borrowed_idle(&mut pooled);
        let result = connection.transaction(f);
        let poisoned = connection.transaction_status().is_poisoned();
        drop(connection);

        // A refused rollback leaves the transaction state unknown, so the
        // checkout is not proven: the guard repairs it on the way out.
        if !poisoned {
            pooled.mark_idle();
        }

        result.map_err(PgPoolError::Query)
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
    /// requested level is ignored for that nested block, because SQL defines
    /// no per-savepoint isolation and the enclosing transaction keeps the
    /// level it was opened with.
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
        let mut connection = PgConnection::borrowed_idle(&mut pooled);
        let result = connection.transaction_with_isolation(isolation, f);
        let poisoned = connection.transaction_status().is_poisoned();
        drop(connection);

        if !poisoned {
            pooled.mark_idle();
        }

        result.map_err(PgPoolError::Query)
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

    /// Reads every row of `query` on a checked-out connection.
    ///
    /// The statement runs in autocommit and the checkout returns the
    /// connection under the pool's autocommit contract: a
    /// transaction-control statement issued here breaks that contract
    /// instead of being refused, and leaves its transaction open on a
    /// session the next checkout inherits.
    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        let mut connection = self.get()?;
        let rows = fetch_raw_sync(&mut connection, query).map_err(PgPoolError::Query);
        // The contract of this path: the statement runs in autocommit, so it
        // leaves the session outside a transaction whether the driver
        // executed it or refused it, and the pool records the idle proof it
        // never verified. A transaction-control statement issued here breaks
        // the contract, which is why this path repairs nothing.
        connection.mark_idle();
        rows
    }

    /// Reads at most `limit` rows from the checked-out connection.
    ///
    /// The limit reaches the row iterator of the underlying `postgres`
    /// client, so a single-row lookup stops after the second row instead of
    /// materialising the whole result set.
    ///
    /// The statement runs in autocommit, under the same contract as the
    /// other statement paths of the pool.
    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        let mut connection = self.get()?;
        let rows = fetch_raw_limited_sync(&mut connection, query, limit)
            .map_err(PgPoolError::Query);
        connection.mark_idle();
        rows
    }

    /// Executes `query` on a checked-out connection and reports the number
    /// of affected rows.
    ///
    /// The statement runs in autocommit and the checkout returns the
    /// connection under the pool's autocommit contract: a
    /// transaction-control statement issued here breaks that contract
    /// instead of being refused, and leaves its transaction open on a
    /// session the next checkout inherits.
    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        let mut connection = self.get()?;
        let affected = execute_raw_sync(&mut connection, query).map_err(PgPoolError::Query);
        connection.mark_idle();
        affected
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
