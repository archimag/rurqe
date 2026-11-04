//! The adapter-facing contracts of the synchronous transaction protocol.
//!
//! A synchronous driver adapter is written against three items:
//! [`TransactionBackend`] renders and executes the transaction-control
//! statements the guard issues, [`TransactionalConnection`] hands the guard the
//! bookkeeping those statements update, and [`IsolationLevel`] names the level
//! the outermost `BEGIN` may request. The guard itself lives in the parent
//! module and is built entirely on these three contracts, so no adapter repeats
//! the savepoint, rollback, or poison protocol.
//!
//! Transaction control binds no parameters, so every default method of
//! [`TransactionBackend`] renders its statement into a [`BuiltQuery`] without
//! params and executes it through the adapter's own [`QueryBackend`]
//! implementation. A savepoint name cannot be bound either, which is why
//! [`SavepointName`] validates the text on the way in: the rendered statement
//! carries the name as a single identifier token.
//!
//! A driver that refuses a transaction-control statement hands its own error
//! back, and what the refusal means belongs to the adapter. SQLite reports a
//! `ROLLBACK` issued with no transaction open as an error, so its adapter
//! answers that refusal as benign and the guard records the connection clean
//! instead of poisoning it.
//!
//! # Session hygiene
//!
//! Two session states turn the following statements into silent lies: a
//! transaction-control statement the driver refused may leave the session
//! holding the transaction it was meant to close, and a transaction a raw
//! driver handle opened is invisible to the bookkeeping of the adapter
//! altogether. In both states the engine accepts the next statement inside the
//! abandoned transaction, whose work survives only if something later commits
//! it. The contracts of this module therefore carry three obligations,
//! discharged by the adapter, the pool, and every build configuration alike.
//!
//! **A recorded refusal closes the bare statement path.** While
//! [`TransactionStatus`] records a refused transaction-control statement, the
//! adapter answers [`fetch_raw`](QueryBackend::fetch_raw),
//! [`fetch_raw_limited`](QueryBackend::fetch_raw_limited), and
//! [`execute_raw`](QueryBackend::execute_raw) with that recorded failure
//! instead of dispatching them, because the session may still hold the
//! transaction the statement could not close. The methods of
//! [`TransactionBackend`] stay callable, because their statements are exactly
//! the ones that end the state.
//!
//! **A typed transaction entry proves or repairs the session.** A transaction a
//! raw driver handle opened is invisible to the record, so the entry points
//! that open a block establish that the session is outside a transaction before
//! [`begin`](TransactionBackend::begin) issues its statement: the caller proves
//! it — a pooled checkout the pool drove to a known ending — or the adapter
//! returns the session to an idle state itself. The recovery of
//! [`TransactionGuard::begin`](super::TransactionGuard::begin) replays the
//! failures its own record holds; it cannot observe a transaction opened behind
//! its back, which is why the proof belongs to the adapter.
//!
//! **A pool settles the disposition of every checkout.** The session of a
//! pooled connection outlives the adapter that drove it, so a pool proves a
//! checkout idle, repairs it, or destroys the connection before it is reused,
//! and a session it cannot vouch for never reaches the next caller.
//!
//! The synchronous PostgreSQL adapter is the reference implementation of the
//! three obligations: `PgConnection` answers a bare statement on a connection
//! recording a refused control statement with `PgError::ConnectionPoisoned`
//! before the driver sees it, its typed transaction entries return the session
//! to an idle state before the block opens, and `PgPooledConnection`, the guard
//! of its `r2d2` pool, repairs a checkout the pool cannot prove. The
//! obligations bind every surface and every build configuration: a statement
//! path that never consults the record, an entry that repairs only a debug
//! build, or a pool that recycles a session it cannot vouch for withdraws the
//! guarantee of the whole protocol on that surface.
use crate::query::{BuiltQuery, QueryBackend, SavepointName};

use super::TransactionStatus;

/// The isolation level a transaction may be opened with.
///
/// The levels are the three SQL standard ones the synchronous adapters expose;
/// a level requested for a nested block is ignored, because SQL defines no
/// per-savepoint isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    /// A statement observes only rows committed before it started.
    ReadCommitted,
    /// Every statement of the transaction observes one snapshot.
    RepeatableRead,
    /// The transaction behaves as if it ran alone.
    Serializable,
}

impl IsolationLevel {
    /// The canonical SQL spelling of the level.
    pub fn as_str(self) -> &'static str {
        match self {
            IsolationLevel::ReadCommitted => "READ COMMITTED",
            IsolationLevel::RepeatableRead => "REPEATABLE READ",
            IsolationLevel::Serializable => "SERIALIZABLE",
        }
    }
}

/// Executes one transaction-control statement without bound parameters.
fn execute_statement<B: QueryBackend + ?Sized>(
    backend: &mut B,
    sql: String,
) -> Result<(), B::Error> {
    backend
        .execute_raw(&BuiltQuery {
            sql,
            params: Vec::new(),
        })
        .map(|_| ())
}

/// The transaction-control statements a driver adapter executes for the guard.
///
/// An implementation renders the control statements of its engine and hands the
/// driver error of a refusal back unchanged, so the guard can classify what the
/// refusal means for the session: a refusal the adapter answers as benign
/// leaves the connection clean, while any other refusal is recorded and closes
/// the bare statement path of that adapter, as the session hygiene obligations
/// of this module describe.
///
/// The methods stay callable while that record is present. The rollback of a
/// dropping guard, the savepoint recovery of the next entry, and the retry
/// performed by [`begin`](TransactionBackend::begin) are the statements that
/// end the state, so a gate that covered them would leave the session
/// unrepairable. [`begin`](TransactionBackend::begin) is also where the second
/// obligation meets the trait: an adapter that offers a typed transaction entry
/// proves the session idle or repairs it before this method issues its
/// statement, so a transaction a raw driver handle left open is never joined.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot run RURQE transaction blocks",
    label = "unsupported transaction backend",
    note = "driver adapters implement `TransactionBackend` by issuing the transaction-control statements of their engine"
)]
pub trait TransactionBackend: QueryBackend {
    /// Opens a transaction, optionally at the requested `isolation` level.
    ///
    /// `None` keeps the default of the engine, and `Some(level)` renders the
    /// level into the opening statement as `BEGIN ISOLATION LEVEL <level>`.
    fn begin(&mut self, isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
        let mut sql = String::from("BEGIN");
        if let Some(level) = isolation {
            sql.push_str(" ISOLATION LEVEL ");
            sql.push_str(level.as_str());
        }
        execute_statement(self, sql)
    }

    /// Commits the open transaction.
    fn commit(&mut self) -> Result<(), Self::Error> {
        execute_statement(self, String::from("COMMIT"))
    }

    /// Rolls the open transaction back.
    fn rollback(&mut self) -> Result<(), Self::Error> {
        execute_statement(self, String::from("ROLLBACK"))
    }

    /// Opens the savepoint `name` inside the open transaction.
    fn savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let mut sql = String::from("SAVEPOINT ");
        sql.push_str(name.as_str());
        execute_statement(self, sql)
    }

    /// Releases the savepoint `name`, keeping its statements.
    fn release_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let mut sql = String::from("RELEASE SAVEPOINT ");
        sql.push_str(name.as_str());
        execute_statement(self, sql)
    }

    /// Rolls back to the savepoint `name`, discarding its statements.
    fn rollback_to_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        let mut sql = String::from("ROLLBACK TO SAVEPOINT ");
        sql.push_str(name.as_str());
        execute_statement(self, sql)
    }
}

/// A connection owning the bookkeeping of the transaction opened on it.
///
/// The guard mutates the bookkeeping through the returned status, so the
/// adapter only has to keep it next to its handle.
///
/// That bookkeeping is the record the session hygiene protocol is keyed on. The
/// adapter consults [`TransactionStatus::is_poisoned`] before it dispatches a
/// bare statement, so a refusal it holds closes the statement path instead of
/// letting the statement join the abandoned transaction, and it reads
/// [`TransactionStatus::is_owned`] to decide what a recovery may touch: the
/// whole transaction a departed guard left behind, or only the savepoint of a
/// transaction a live guard still owns.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot own a RURQE transaction",
    label = "unsupported transactional connection",
    note = "driver adapters implement `TransactionalConnection` for the handle that keeps the transaction status"
)]
pub trait TransactionalConnection: QueryBackend + TransactionBackend {
    /// The bookkeeping of the transaction currently open on the connection.
    fn transaction_status(&mut self) -> &mut TransactionStatus<Self::Error>;
}
