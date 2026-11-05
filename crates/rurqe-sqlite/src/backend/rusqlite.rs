//! The `rusqlite` connection adapter.

use core::borrow::{Borrow, BorrowMut};
use core::ops::{Deref, DerefMut};

use ::rusqlite::Connection;

use rurqe_core::dialect::Dialect;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{
    BuiltQuery, IsolationLevel, QueryBackend, SavepointName, TransactionBackend, TransactionGuard,
    TransactionStatus, TransactionalConnection,
};

use crate::dialect::{Sqlite, Sqlite3_35};

use super::types::{value_ref_to_sql_val, SqlValParam};

/// A `rusqlite` handle that executes RURQE-built queries.
///
/// The adapter is generic over the handle it executes through, and the default
/// type parameter is an owned [`Connection`]: `SqliteConnection` therefore keeps
/// naming the adapter that owns its connection. [`SqliteConnection::borrowed`]
/// builds the same adapter over a handle the caller keeps owning, so a
/// connection checked out of a pool is driven through the identical contracts —
/// guarded `BEGIN`/`COMMIT`, `SAVEPOINT` nesting, and rollback on error — and
/// returns to its pool once the adapter is dropped.
///
/// The adapter implements the RURQE
/// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
/// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
/// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available.
/// It also dereferences to the wrapped [`Connection`], so raw SQL can still be
/// executed directly.
///
/// The adapter refuses every bare statement while its bookkeeping records a
/// poison, which is the state a refused transaction-control statement leaves
/// behind: the session may still hold the transaction whose statement failed,
/// so a statement issued there would join the abandoned transaction, whose
/// work disappears with the rollback that eventually closes it. The guard
/// protocol keeps running through the transaction-control path, because the
/// rollback a dropping scope issues and the retry of the next transaction
/// entry are exactly the statements that end the state.
///
/// The other half of the same hazard is a transaction the adapter never
/// recorded: a raw driver handle opens one through the supported escape hatch,
/// so the next typed transaction entry would join it — SQLite refuses the
/// `BEGIN` outright — and a bare statement would run inside work nobody
/// commits. Every typed transaction entry therefore returns an unproven session
/// to an idle state before its block opens.
pub struct SqliteConnection<B = Connection> {
    connection: B,
    status: TransactionStatus<::rusqlite::Error>,
}

impl SqliteConnection<Connection> {
    /// Wraps an existing `rusqlite` connection.
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            status: TransactionStatus::new(),
        }
    }

    /// Opens a new in-memory SQLite database.
    pub fn open_in_memory() -> Result<Self, ::rusqlite::Error> {
        Connection::open_in_memory().map(Self::new)
    }

    /// Consumes the adapter, returning the wrapped connection.
    pub fn into_inner(self) -> Connection {
        self.connection
    }
}

impl SqliteConnection<&mut Connection> {
    /// Wraps a borrowed connection handle with fresh transaction bookkeeping.
    ///
    /// The handle stays owned by the caller, which is what makes the adapter
    /// usable over a connection checked out of a pool: the handle is borrowed
    /// for the lifetime of the adapter and returns to its owner once the
    /// adapter is dropped. The bookkeeping starts clean, because such a handle
    /// may have executed statements before it was checked out.
    pub fn borrowed(connection: &mut Connection) -> SqliteConnection<&mut Connection> {
        SqliteConnection {
            connection,
            status: TransactionStatus::new(),
        }
    }
}

impl<B: Borrow<Connection>> SqliteConnection<B> {
    /// Returns a shared reference to the wrapped connection.
    pub fn connection(&self) -> &Connection {
        self.connection.borrow()
    }
}

impl<B: BorrowMut<Connection>> SqliteConnection<B> {
    /// Returns a mutable reference to the wrapped connection.
    ///
    /// The accessor is a supported escape hatch for behaviour the typed API does
    /// not model, on par with the raw SQL macros: the statements issued through it
    /// stay outside the transaction bookkeeping of the adapter, so a manual
    /// `BEGIN` leaves the connection inside a transaction the adapter never
    /// records. The caller owns that state and leaves the connection idle when it
    /// is done. A session the caller leaves open is rolled back at the next typed
    /// transaction entry, before its block opens, in every build configuration,
    /// and the checkout of the pool repairs it on the way out, so the leak is
    /// contained instead of degrading the connection until it is discarded.
    pub fn connection_mut(&mut self) -> &mut Connection {
        self.connection.borrow_mut()
    }

    /// Runs `f` inside a transaction, committing on `Ok` and rolling back on `Err`.
    ///
    /// When no transaction is open the block runs between `BEGIN` and `COMMIT`.
    /// Calling `transaction` again while a transaction is already open nests the
    /// block inside a `SAVEPOINT`, so an inner failure rolls back only its own
    /// statements while the enclosing transaction stays intact. A panic raised by
    /// `f` is rolled back as well, so the transaction can never be left
    /// half-finished.
    ///
    /// The session is returned to an idle state before the block opens, so a
    /// transaction a raw driver handle left open is rolled back here instead of
    /// making the `BEGIN` of the block fail. A session the bookkeeping records as
    /// open or poisoned is left to the recovery of the guard itself: a nested
    /// block belongs to the enclosing transaction, and a poisoned connection is
    /// repaired by retrying the statement the driver refused.
    pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut SqliteConnection<B>) -> Result<T, E>,
        E: From<::rusqlite::Error>,
    {
        self.repair_idle_session().map_err(E::from)?;
        let mut guard = TransactionGuard::begin(self).map_err(E::from)?;
        let value = f(&mut guard)?;
        guard.commit().map_err(E::from)?;
        Ok(value)
    }

    /// Answers a refused transaction-control statement as benign when the
    /// driver observes no open transaction.
    ///
    /// The raw handle of the adapter is a supported escape hatch, so a caller
    /// may issue `COMMIT` or `ROLLBACK` through it while a typed scope is
    /// still open. SQLite then refuses the statement the guard sends next with
    /// `cannot rollback - no transaction is active`, even though the
    /// connection is already exactly as clean as the guard wanted it to be, so
    /// that refusal must not be recorded as a poisoning failure. The
    /// observation is `is_autocommit` — the same one the CLI migration driver
    /// uses to detect a script that left a transaction open: a refusal raised
    /// while no transaction is open is benign, while one that leaves a
    /// transaction open stays the driver's own error.
    fn benign_if_closed(&self, error: ::rusqlite::Error) -> Result<(), ::rusqlite::Error> {
        if self.connection.borrow().is_autocommit() {
            Ok(())
        } else {
            Err(error)
        }
    }

    /// Returns a session an untyped driver handle left inside a transaction to an
    /// idle state.
    ///
    /// A typed entry point is the only place where the adapter can still tell
    /// that the caller is about to open a transaction of its own, so the session
    /// is handed to [`ensure_idle`] here, in every build configuration: SQLite
    /// refuses `BEGIN` inside an open transaction, so a leak through the raw
    /// handle would otherwise degrade the connection until it is discarded and
    /// the work of the block would never run.
    ///
    /// The repair runs only while the bookkeeping records neither an open
    /// transaction nor a poisoning: a nested savepoint block belongs to the
    /// enclosing transaction and must not be reset, and a poisoned connection
    /// belongs to the recovery of [`TransactionGuard`], which retries the
    /// statement the driver refused instead of closing the transaction behind its
    /// back. The observation is the engine's own autocommit flag, so a session
    /// that is already idle pays no statement at all.
    fn repair_idle_session(&mut self) -> Result<(), ::rusqlite::Error> {
        if self.status.is_open() || self.status.is_poisoned() {
            return Ok(());
        }
        ensure_idle(self.connection.borrow_mut())
    }
}

impl<B> SqliteConnection<B> {
    /// Reports whether the connection is poisoned and refuses bare statements.
    ///
    /// The state is recorded when the adapter can no longer prove that the
    /// session is outside a transaction: a rollback, a savepoint rollback, or
    /// a savepoint release the driver refused while the transaction it was
    /// meant to close stayed open leaves the work of the block pending. A
    /// statement issued on such a session would join the abandoned
    /// transaction, whose work disappears when it is finally rolled back,
    /// while the caller observed the statement succeed. The adapter therefore
    /// refuses every bare statement before it reaches the driver, and a caller
    /// holding the handle detects the state here instead of observing work
    /// nobody keeps.
    ///
    /// The state is not permanent, because the connection can still close what
    /// it could not undo: the next typed transaction entry retries the refused
    /// control statement before its block opens, and a retry the driver accepts
    /// clears the record. A caller therefore reads `false` once the abandoned
    /// transaction is closed for real, and the connection serves work again
    /// without being discarded.
    pub fn is_poisoned(&self) -> bool {
        self.status.is_poisoned()
    }

    /// Refuses to execute a statement while the connection records a poison.
    ///
    /// The adapter can no longer prove that the session is outside the
    /// transaction whose control statement the driver refused, so a bare
    /// statement would run inside the abandoned transaction: SQLite accepts it
    /// there, and the work disappears with the rollback that eventually closes
    /// that transaction, while the caller observed it succeed. The refusal
    /// carries the recorded driver failure, and the recovery a synchronous
    /// connection offers is the next typed transaction entry, which retries the
    /// abandoned statement instead of joining it.
    fn refuse_if_poisoned(&self) -> Result<(), ::rusqlite::Error> {
        match self.status.poison() {
            Some(error) => Err(poison_refusal(error)),
            None => Ok(()),
        }
    }
}

/// Builds the refusal reported for a bare statement on a poisoned connection.
///
/// The crate reports an adapter-side refusal through the driver's conversion
/// failure carrying a `std::io::Error` that names the reason, exactly as the
/// `NaN` and non-finite-array parameter refusals do, so the public error type
/// of the adapter stays the driver's own. The reason names the recorded
/// transaction-control failure and the recovery the next typed transaction
/// entry performs.
fn poison_refusal(error: &::rusqlite::Error) -> ::rusqlite::Error {
    let cause = ::std::io::Error::other(format!(
        "the connection recorded a refused transaction-control statement ({error}); \
         the next typed transaction retries it before its block opens"
    ));
    ::rusqlite::Error::ToSqlConversionFailure(Box::new(cause))
}

/// Renders a savepoint name as a quoted SQLite identifier.
///
/// A savepoint name cannot be bound as a parameter, so it is rendered into the
/// statement text. [`SavepointName`] restricts the text to `[A-Za-z0-9_]`, but
/// that invariant belongs to `rurqe-core` and is invisible here, so the name is
/// quoted through the SQLite identifier rule instead of being concatenated raw:
/// an embedded `"` is doubled and the whole name stays a single token. A quoted
/// name parses to the same savepoint text as the bare token the shared
/// `SAVEPOINT` statement creates, so both spellings address one savepoint, and
/// identifier escaping is identical across every declared SQLite version.
fn quoted_savepoint(name: &str) -> String {
    Sqlite::<Sqlite3_35>::quote_identifier(name)
}
/// Returns the session of a connection the adapter cannot vouch for to a
/// provably idle state.
///
/// Two callers need the same observation: the checkout of a pool, which hands a
/// recycled session to the next block, and the entry of a typed transaction,
/// which opens a block of its own on a session a raw driver handle may have left
/// inside one.
///
/// A connection recycled by a pool carries the session state of the block that
/// drove it last: a transaction a raw handle opened, a `BEGIN` whose `COMMIT`
/// the caller never issued, or a block whose rollback the driver refused. The
/// bookkeeping of an adapter dies with the adapter, while the session of the
/// connection outlives it, so a checkout is where that state can still be
/// closed.
///
/// SQLite reports the state directly: the autocommit flag is true exactly while
/// the connection is outside a transaction, so a session that is already idle
/// issues no statement at all, and a session that still holds a transaction has
/// it ended by a single `ROLLBACK`. The repair is therefore sound by the
/// observation rather than by classifying the statements that arrived, and it is
/// the same observation the adapter uses to classify a rollback the driver
/// refused.
///
/// A reset the driver refuses means the connection could not be brought back to
/// a known state, so the failure is reported instead of handing the connection
/// to a caller that would run its work inside the abandoned transaction.
pub(crate) fn ensure_idle(connection: &mut Connection) -> Result<(), ::rusqlite::Error> {
    if connection.is_autocommit() {
        return Ok(());
    }
    connection.execute_batch("ROLLBACK")
}

impl<B: BorrowMut<Connection>> TransactionalConnection for SqliteConnection<B> {
    fn transaction_status(&mut self) -> &mut TransactionStatus<::rusqlite::Error> {
        &mut self.status
    }
}

impl<B: BorrowMut<Connection>> TransactionBackend for SqliteConnection<B> {
    /// Opens a transaction outside the poison gate.
    ///
    /// The statement is executed directly on the wrapped connection, because
    /// the transaction entry is where a poisoned connection recovers: the
    /// shared guard retries the refused control statement here, and a gate on
    /// this path would leave the state unrepairable. SQLite serialises access
    /// through database-level locking, so the requested isolation level does
    /// not map onto a `BEGIN` clause.
    fn begin(&mut self, _isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
        self.connection.borrow_mut().execute_batch("BEGIN")
    }

    /// Commits the open transaction outside the poison gate.
    ///
    /// The guard refuses the commit of a poisoned connection before it reaches
    /// this method, and the rollback a dropping guard issues afterwards is one
    /// of the statements that ends the state, so the statement itself is never
    /// gated.
    fn commit(&mut self) -> Result<(), Self::Error> {
        self.connection.borrow_mut().execute_batch("COMMIT")
    }

    /// Rolls the transaction back, tolerating a transaction the raw handle
    /// already closed.
    ///
    /// A refused `ROLLBACK` is benign exactly when the driver observes no open
    /// transaction, so the transaction bookkeeping records the connection
    /// clean instead of poisoning a healthy adapter. `commit` stays untouched:
    /// the ratified classification covers the rollback path only, so a
    /// desynchronized `COMMIT` keeps costing its own transaction call its
    /// driver error.
    fn rollback(&mut self) -> Result<(), Self::Error> {
        let refused = self.connection.borrow_mut().execute_batch("ROLLBACK");
        match refused {
            Ok(()) => Ok(()),
            Err(error) => self.benign_if_closed(error),
        }
    }

    /// Opens the savepoint `name` outside the poison gate.
    ///
    /// A nested block opens its savepoint through this method after the
    /// recovery of the entry returned, so the statement belongs to the control
    /// protocol that ends a poisoned state rather than to the bare statements
    /// it refuses. The name is rendered as a quoted SQLite identifier, so
    /// every savepoint statement of this adapter renders its name through the
    /// identifier rule instead of depending on the character set
    /// `rurqe-core` validates the name against.
    fn savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let statement = format!("SAVEPOINT {}", quoted_savepoint(name.as_str()));
        self.connection.borrow_mut().execute_batch(&statement)
    }

    /// Rolls back to the savepoint `name`, tolerating a savepoint the raw
    /// handle already closed along with the transaction that carried it.
    ///
    /// The name is rendered as a quoted SQLite identifier instead of being
    /// concatenated raw, so the statement text of this adapter never depends on
    /// the character set `rurqe-core` validates the name against.
    ///
    /// The refusal is benign under the same observation as
    /// [`rollback`](TransactionBackend::rollback): no open transaction means
    /// there is no abandoned work left to report.
    fn rollback_to_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let statement = format!("ROLLBACK TO SAVEPOINT {}", quoted_savepoint(name.as_str()));
        let refused = self.connection.borrow_mut().execute_batch(&statement);
        match refused {
            Ok(()) => Ok(()),
            Err(error) => self.benign_if_closed(error),
        }
    }

    /// Releases the savepoint `name`, tolerating a savepoint the raw handle
    /// already closed along with the transaction that carried it.
    ///
    /// The name is rendered as a quoted SQLite identifier, exactly as in
    /// [`rollback_to_savepoint`](TransactionBackend::rollback_to_savepoint).
    ///
    /// The refusal is benign under the same observation as
    /// [`rollback`](TransactionBackend::rollback): a block whose savepoint is
    /// already gone holds no work the release could keep.
    fn release_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let statement = format!("RELEASE SAVEPOINT {}", quoted_savepoint(name.as_str()));
        let refused = self.connection.borrow_mut().execute_batch(&statement);
        match refused {
            Ok(()) => Ok(()),
            Err(error) => self.benign_if_closed(error),
        }
    }
}

impl<B: Borrow<Connection>> Deref for SqliteConnection<B> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection.borrow()
    }
}

impl<B: BorrowMut<Connection>> DerefMut for SqliteConnection<B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.borrow_mut()
    }
}

impl<B: BorrowMut<Connection>> QueryBackend for SqliteConnection<B> {
    type Error = ::rusqlite::Error;

    /// Reads every row of `query` on a session the adapter can vouch for.
    ///
    /// A poisoned connection refuses the statement before it reaches the
    /// driver, because the session may still hold the transaction whose
    /// control statement failed: the rows would otherwise observe work nobody
    /// keeps.
    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        self.refuse_if_poisoned()?;
        fetch_raw_with(self.connection.borrow_mut(), query)
    }

    /// Reads at most `limit` rows of `query` on a session the adapter can
    /// vouch for, refusing a poisoned connection exactly as
    /// [`fetch_raw`](QueryBackend::fetch_raw) does.
    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        self.refuse_if_poisoned()?;
        fetch_raw_limited_with(self.connection.borrow_mut(), query, limit)
    }

    /// Executes `query` on a session the adapter can vouch for.
    ///
    /// A poisoned connection refuses the statement before it reaches the
    /// driver: a write would otherwise join the abandoned transaction and
    /// disappear with the rollback that eventually closes it, while the caller
    /// observed it succeed.
    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        self.refuse_if_poisoned()?;
        execute_raw_with(self.connection.borrow_mut(), query)
    }
}

/// Fetches the rows of `query` through a raw `rusqlite` connection.
pub(super) fn fetch_raw_with(
    connection: &mut Connection,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, ::rusqlite::Error> {
    fetch_raw_limited_with(connection, query, None)
}

/// Fetches at most `limit` rows of `query` through a raw `rusqlite` connection.
///
/// The row loop stops as soon as `limit` rows have been read, so the remaining
/// rows of an unbounded result set are never stepped over or decoded.
pub(super) fn fetch_raw_limited_with(
    connection: &mut Connection,
    query: &BuiltQuery,
    limit: Option<usize>,
) -> Result<Vec<Vec<SqlVal>>, ::rusqlite::Error> {
    let mut statement = connection.prepare(&query.sql)?;
    let column_count = statement.column_count();
    let params = query.params.iter().map(SqlValParam);
    let mut rows = statement.query(::rusqlite::params_from_iter(params))?;

    let limit = limit.unwrap_or(usize::MAX);
    let mut result = Vec::new();
    while result.len() < limit {
        let Some(row) = rows.next()? else {
            break;
        };
        let mut values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            values.push(value_ref_to_sql_val(index, row.get_ref(index)?)?);
        }
        result.push(values);
    }
    Ok(result)
}

/// Executes `query` through a raw `rusqlite` connection.
pub(super) fn execute_raw_with(
    connection: &mut Connection,
    query: &BuiltQuery,
) -> Result<u64, ::rusqlite::Error> {
    let params = query.params.iter().map(SqlValParam);
    let affected = connection.execute(&query.sql, ::rusqlite::params_from_iter(params))?;
    Ok(affected as u64)
}

#[cfg(test)]
mod tests {
    use super::{quoted_savepoint, SqliteConnection};

    #[test]
    fn a_generated_name_renders_as_a_quoted_identifier() {
        assert_eq!(quoted_savepoint("rurqe_sp_7"), "\"rurqe_sp_7\"");
    }

    #[test]
    fn metacharacters_stay_inside_the_quoted_identifier() {
        // The validation of `SavepointName` rejects these characters, and the
        // statements are still rendered so that a name which ever did carry one
        // reaches SQLite as a single escaped token instead of as SQL.
        assert_eq!(
            quoted_savepoint("sp\"; DROP TABLE users; --"),
            "\"sp\"\"; DROP TABLE users; --\""
        );
        assert_eq!(quoted_savepoint("sp'or"), "\"sp'or\"");
    }

    /// Opens an in-memory adapter holding one fixture table.
    fn database() -> SqliteConnection {
        let connection = SqliteConnection::open_in_memory().expect("open in-memory database");
        connection
            .execute_batch("CREATE TABLE account (id INTEGER PRIMARY KEY)")
            .expect("create the fixture table");
        connection
    }

    /// Counts the rows of the fixture table through the wrapped handle.
    fn accounts(connection: &SqliteConnection) -> i64 {
        connection
            .connection()
            .query_row(
                "SELECT count(*) FROM account",
                ::rusqlite::params![],
                |row| row.get::<_, i64>(0),
            )
            .expect("count the rows of the fixture table")
    }

    #[test]
    fn a_raw_transaction_is_rolled_back_before_the_block_opens() {
        let mut db = database();

        // The raw handle is a supported escape hatch, so the transaction it opens
        // stays outside the bookkeeping of the adapter.
        db.connection_mut()
            .execute_batch("BEGIN")
            .expect("the raw handle opens a transaction");
        db.connection_mut()
            .execute_batch("INSERT INTO account (id) VALUES (1)")
            .expect("the raw insert runs inside it");
        assert!(
            !db.connection().is_autocommit(),
            "the raw transaction is open"
        );

        db.transaction(|tx| tx.execute_batch("INSERT INTO account (id) VALUES (2)"))
            .expect("the typed block opens on a repaired session");

        assert_eq!(
            accounts(&db),
            1,
            "the abandoned transaction was rolled back and only the block committed"
        );
    }

    #[test]
    fn a_nested_block_never_repairs_the_enclosing_transaction() {
        let mut db = database();

        db.transaction(|tx| -> Result<(), ::rusqlite::Error> {
            tx.execute_batch("INSERT INTO account (id) VALUES (1)")?;
            let nested = tx.transaction(|inner| {
                inner.execute_batch("INSERT INTO account (id) VALUES (2)")?;
                Err::<(), _>(::rusqlite::Error::InvalidQuery)
            });
            assert!(nested.is_err(), "the nested block reports its own error");
            Ok(())
        })
        .expect("the enclosing block commits");

        assert_eq!(
            accounts(&db),
            1,
            "only the enclosing insert survived the nested rollback"
        );
    }
}
