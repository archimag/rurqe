//! Synchronous execution adapter over a [`postgres::Client`] handle.

use std::borrow::{Borrow, BorrowMut};

use postgres::fallible_iterator::FallibleIterator;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{
    BuiltQuery, IsolationLevel, QueryBackend, TransactionBackend, TransactionGuard,
    TransactionStatus, TransactionalConnection,
};

use super::types::{BoundParams, PgError, RowDecoder};

/// Executes `query` through a synchronous client without decoding rows.
pub(crate) fn fetch_raw_sync(
    client: &mut postgres::Client,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, PgError> {
    fetch_raw_limited_sync(client, query, None)
}

/// Executes `query` through a synchronous client and returns affected rows.
pub(crate) fn execute_raw_sync(
    client: &mut postgres::Client,
    query: &BuiltQuery,
) -> Result<u64, PgError> {
    // The parameters are prepared once and handed over as references, so the
    // statement pays for one buffer of its own values and never for a copy of
    // them. The statement API of the synchronous driver takes those references
    // as one slice, so this is the one entry point that collects the reference
    // buffer that call itself requires; a statement without parameters
    // collects an empty buffer and allocates nothing at all.
    let params = BoundParams::of(query);
    let references = params.references();
    let affected = client.execute(query.sql.as_str(), &references)?;
    Ok(affected)
}

/// Executes `query` through a synchronous client and decodes at most `limit`
/// rows, stopping the adapter's iteration once the limit is reached.
///
/// Without a limit the whole result set is fetched, exactly as
/// [`fetch_raw_sync`] does.
///
/// The bound saves decoding and materialisation, never server work. The wire
/// protocol requires the response of a statement to be consumed through
/// `ReadyForQuery` before the connection can read the response of the next
/// command, so the driver keeps reading the `DataRow` messages past the limit
/// and discards them. The next statement on that connection therefore completes
/// only after the drain, which matters for a pooled connection: a synchronous
/// pooled handle (`PgPool`) returns to the pool as soon as the fetch completes,
/// and the caller that checks the connection out next waits for the drain.
/// Truncating the work the server performs would take an explicit cursor
/// (`DECLARE` / `FETCH`), which this adapter does not open.
///
/// Every row is decoded as it arrives instead of being collected first, so the
/// message buffer of a row is released before the next one is read.
pub(crate) fn fetch_raw_limited_sync(
    client: &mut postgres::Client,
    query: &BuiltQuery,
    limit: Option<usize>,
) -> Result<Vec<Vec<SqlVal>>, PgError> {
    // The parameters are prepared once and handed over as references, so the
    // statement pays for one buffer of its own values and never for a copy of
    // them.
    let params = BoundParams::of(query);
    let mut decoder = RowDecoder::new();
    let mut rows = Vec::new();
    let mut stream = client.query_raw(query.sql.as_str(), params.bindings())?;
    let ceiling = limit.unwrap_or(usize::MAX);
    while rows.len() < ceiling {
        match stream.next()? {
            Some(row) => rows.push(decoder.decode(&row)?),
            None => break,
        }
    }
    Ok(rows)
}

/// Returns the session of a checked-out client to a provably idle state.
///
/// A connection recycled by a pool carries the session state of the block that
/// drove it last: a transaction a raw handle opened, a `BEGIN` whose `COMMIT`
/// the caller never issued, or a block whose rollback the driver refused. The
/// bookkeeping of an adapter dies with the adapter, while the session of the
/// server outlives it, so a checkout is the place where that state can still
/// be closed.
///
/// The reset is one round trip of the simple protocol carrying `BEGIN;
/// ROLLBACK`. Opening the transaction first makes the trailing `ROLLBACK`
/// legitimate in every session state, so a checkout that finds an idle session
/// logs nothing at all, while a session that still holds a transaction has it
/// ended by the `ROLLBACK`. The idle handout is therefore guaranteed by
/// construction instead of by classifying the state the session arrived in,
/// and the only trace the reset leaves is the server-side warning it emits
/// when it actually repairs an abandoned transaction.
///
/// A reset the driver refuses means the connection could not be brought back
/// to a known state, so the failure is reported instead of handing the
/// connection to a block that would run inside the abandoned transaction.
#[cfg_attr(not(any(feature = "r2d2", debug_assertions)), allow(dead_code))]
pub(crate) fn ensure_idle(client: &mut postgres::Client) -> Result<(), PgError> {
    client
        .batch_execute("BEGIN; ROLLBACK")
        .map_err(|failure| PgError::AbandonedTransaction {
            detail: failure.to_string(),
        })
}

/// A synchronous PostgreSQL handle executing RURQE queries.
///
/// The adapter is generic over the handle it executes through, and the default
/// type parameter is an owned [`postgres::Client`]: `PgConnection` therefore
/// keeps naming the adapter that owns its connection. [`PgConnection::borrowed`]
/// builds the same adapter over a handle the caller keeps owning, so a
/// connection checked out of a pool is driven through the identical contracts —
/// guarded `BEGIN`/`COMMIT`, `SAVEPOINT` nesting, and rollback on error — and
/// returns to its pool once the adapter is dropped.
///
/// The adapter implements the RURQE
/// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
/// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
/// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available.
pub struct PgConnection<B = postgres::Client> {
    client: B,
    status: TransactionStatus<PgError>,
}

impl PgConnection<postgres::Client> {
    /// Wraps an existing synchronous PostgreSQL client.
    pub fn new(client: postgres::Client) -> Self {
        Self {
            client,
            status: TransactionStatus::new(),
        }
    }

    /// Consumes the adapter, returning the wrapped client.
    pub fn into_inner(self) -> postgres::Client {
        self.client
    }
}

impl PgConnection<&mut postgres::Client> {
    /// Wraps a borrowed client handle with fresh transaction bookkeeping.
    ///
    /// The handle stays owned by the caller, which is what makes the adapter
    /// usable over a connection checked out of a pool: the handle is borrowed
    /// for the lifetime of the adapter and returns to its owner once the
    /// adapter is dropped. The bookkeeping starts clean, because such a handle
    /// may have executed statements before it was checked out.
    ///
    /// The constructor trusts the handle it receives: the bookkeeping starts
    /// clean because a pooled checkout has already returned the session to an
    /// idle state before handing the connection out, so the adapter does not
    /// probe the driver for the state of that session. A caller that wraps a
    /// foreign handle directly — one that never passed through a pooled
    /// checkout — verifies the idleness of that session itself.
    pub fn borrowed(client: &mut postgres::Client) -> PgConnection<&mut postgres::Client> {
        PgConnection {
            client,
            status: TransactionStatus::new(),
        }
    }
}

impl<B: Borrow<postgres::Client>> PgConnection<B> {
    /// Returns a shared reference to the wrapped client.
    pub fn client(&self) -> &postgres::Client {
        self.client.borrow()
    }
}

impl<B: BorrowMut<postgres::Client>> PgConnection<B> {
    /// Returns a mutable reference to the wrapped client.
    ///
    /// The accessor is a supported escape hatch for behaviour the typed API
    /// does not model, on par with the raw SQL macros: the statements issued
    /// through it stay outside the transaction bookkeeping of the adapter, so
    /// a manual `BEGIN` leaves the connection inside a transaction the adapter
    /// never records. The caller owns that state and leaves the connection
    /// idle when it is done. A session that is not repaired is reset at the
    /// next pooled checkout and, in a debug build, at the next typed
    /// transaction entry, so the leak is contained instead of being inherited
    /// by the following caller.
    pub fn client_mut(&mut self) -> &mut postgres::Client {
        self.client.borrow_mut()
    }

    /// Runs `f` inside a transaction, committing on `Ok` and rolling back on `Err`.
    ///
    /// The outermost call executes its block between `BEGIN` and `COMMIT`; any
    /// error returned by `f` rolls the transaction back, so a failed block never
    /// leaves partial work behind. Calling `transaction` again while a
    /// transaction is already open nests the block inside a `SAVEPOINT`, so an
    /// inner failure rolls back only its own statements while the enclosing
    /// transaction stays intact. A panicking block is rolled back as well, so
    /// unwinding never leaves an open transaction or savepoint behind.
    ///
    /// A debug build returns the session to an idle state before the block
    /// opens, so a transaction a raw driver handle left open is rolled back
    /// here instead of enclosing the block; a release build keeps the
    /// tolerance the raw handles are documented with.
    pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut PgConnection<B>) -> Result<T, E>,
        E: From<PgError>,
    {
        #[cfg(debug_assertions)]
        self.repair_idle_session().map_err(E::from)?;
        let mut guard = TransactionGuard::begin(self).map_err(E::from)?;
        let value = f(&mut guard)?;
        guard.commit().map_err(E::from)?;
        Ok(value)
    }

    /// Runs `f` inside a transaction at the requested `isolation` level,
    /// committing on `Ok` and rolling back on `Err`.
    ///
    /// The helper mirrors [`transaction`](PgConnection::transaction) and differs
    /// only in the statement that opens the outermost block: the block runs
    /// between `BEGIN ISOLATION LEVEL <level>` and `COMMIT`, with the level text
    /// coming from [`IsolationLevel::as_str`], so every statement of `f` observes
    /// the isolation the caller asked for. [`transaction`](PgConnection::transaction)
    /// keeps the backend default instead, so callers that need no specific level
    /// stay unchanged.
    ///
    /// Calling the helper while a transaction is already open nests the block
    /// inside a `SAVEPOINT`, exactly as [`transaction`](PgConnection::transaction)
    /// does, and the requested level is ignored for that nested block: SQL
    /// defines no per-savepoint isolation, so the enclosing transaction keeps the
    /// level it was opened with. The level therefore applies to the outermost
    /// block only, which is the contract
    /// [`TransactionGuard::begin_with`](rurqe_core::query::TransactionGuard::begin_with)
    /// implements.
    ///
    /// A block returning `Err` and a panicking block are both rolled back, so
    /// neither leaves partial work behind. A driver that refuses that rollback
    /// leaves the connection poisoned, so the next transaction retries the
    /// abandoned rollback instead of running on top of an unknown one.
    ///
    /// ```
    /// use rurqe_core::query::IsolationLevel;
    /// use rurqe_postgres::{PgConnection, PgError};
    ///
    /// // The block runs between `BEGIN ISOLATION LEVEL SERIALIZABLE` and
    /// // `COMMIT`, so no concurrent transaction committing between two
    /// // statements of the block can change what the next one observes.
    /// fn archive_invoices(connection: &mut PgConnection) -> Result<(), PgError> {
    ///     connection.transaction_with_isolation(IsolationLevel::Serializable, |_tx| Ok(()))
    /// }
    /// ```
    pub fn transaction_with_isolation<T, E, F>(
        &mut self,
        isolation: IsolationLevel,
        f: F,
    ) -> Result<T, E>
    where
        F: FnOnce(&mut PgConnection<B>) -> Result<T, E>,
        E: From<PgError>,
    {
        #[cfg(debug_assertions)]
        self.repair_idle_session().map_err(E::from)?;
        let mut guard = TransactionGuard::begin_with(self, Some(isolation)).map_err(E::from)?;
        let value = f(&mut guard)?;
        guard.commit().map_err(E::from)?;
        Ok(value)
    }
    /// Repairs a session left inside a transaction by a raw driver handle.
    ///
    /// A typed entry point is the only place where the adapter can still tell
    /// that the caller is about to open a transaction of its own, so a debug
    /// build returns the session to an idle state here. The repair runs only
    /// while the bookkeeping records neither an open transaction nor a
    /// poisoning: a nested savepoint block belongs to the transaction the
    /// enclosing guard opened and must not be reset, and a poisoned connection
    /// belongs to the recovery of [`TransactionGuard`], which retries the
    /// rollback the driver refused instead of closing the transaction behind
    /// its back.
    ///
    /// A release build skips the repair and keeps the tolerance the raw driver
    /// handles are documented with: the reset is a development aid, not a
    /// guarantee of the typed API.
    #[cfg(debug_assertions)]
    fn repair_idle_session(&mut self) -> Result<(), PgError> {
        if self.status.is_open() || self.status.is_poisoned() {
            return Ok(());
        }
        ensure_idle(self.client.borrow_mut())
    }
}

impl<B: BorrowMut<postgres::Client>> QueryBackend for PgConnection<B> {
    type Error = PgError;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_sync(self.client.borrow_mut(), query)
    }

    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_limited_sync(self.client.borrow_mut(), query, limit)
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, PgError> {
        execute_raw_sync(self.client.borrow_mut(), query)
    }
}

impl<B: BorrowMut<postgres::Client>> TransactionBackend for PgConnection<B> {}

impl<B: BorrowMut<postgres::Client>> TransactionalConnection for PgConnection<B> {
    fn transaction_status(&mut self) -> &mut TransactionStatus<PgError> {
        &mut self.status
    }
}
