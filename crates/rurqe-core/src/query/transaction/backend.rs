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
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot own a RURQE transaction",
    label = "unsupported transactional connection",
    note = "driver adapters implement `TransactionalConnection` for the handle that keeps the transaction status"
)]
pub trait TransactionalConnection: QueryBackend + TransactionBackend {
    /// The bookkeeping of the transaction currently open on the connection.
    fn transaction_status(&mut self) -> &mut TransactionStatus<Self::Error>;
}
