//! Nestable transaction scopes shared by the synchronous driver adapters.
//!
//! Every synchronous adapter offers the same contract: the outermost
//! `transaction` call runs its block between `BEGIN` and `COMMIT`, a call made
//! while a transaction is already open wraps its block in a `SAVEPOINT`, and a
//! failing or unwinding block is rolled back. [`TransactionGuard`] implements
//! that contract once on top of [`TransactionBackend`] and
//! [`TransactionStatus`], so an adapter only exposes its bookkeeping and never
//! repeats the guard.
//!
//! A rollback the driver refuses leaves its transaction open, so the guard never
//! reports such a block as closed: it poisons the connection instead, and the
//! next [`TransactionGuard::begin`] retries the abandoned rollback rather than
//! running a fresh transaction on top of an unknown one.

use core::ops::{Deref, DerefMut};

use super::execution::{TransactionBackend, TransactionContext};
use super::savepoint::SavepointName;

/// The transaction bookkeeping a synchronous connection keeps between calls.
#[derive(Debug, Clone, Default)]
pub struct TransactionStatus {
    context: TransactionContext,
    open: bool,
    poisoned: bool,
}

impl TransactionStatus {
    /// Creates the bookkeeping of a connection without an open transaction.
    pub fn new() -> Self {
        TransactionStatus::default()
    }

    /// Whether the outermost transaction block is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether a failed rollback left the connection in an unknown state.
    ///
    /// A transaction the driver refused to roll back may still hold the work of
    /// its block, so the connection stays poisoned until that transaction is
    /// closed for real: until then the guard refuses to open a new one.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// The number of nested savepoints currently open.
    pub fn depth(&self) -> usize {
        self.context.depth()
    }

    /// The name of the innermost open savepoint, if any.
    pub fn current_savepoint(&self) -> Option<&str> {
        self.context.current_savepoint()
    }

    /// Records whether the outermost transaction block is open.
    fn mark_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Records that a rollback failed, leaving the connection in an unknown state.
    fn mark_poisoned(&mut self) {
        self.poisoned = true;
    }

    /// Records that the connection left the outermost transaction block.
    ///
    /// Closing the outermost transaction discards its savepoints on the server,
    /// so the bookkeeping drops them as well instead of keeping names that the
    /// connection never opened in the first place.
    fn mark_closed(&mut self) {
        self.open = false;
        self.poisoned = false;
        while self.context.close_savepoint().is_some() {}
    }

    /// Allocates a fresh savepoint name and makes it the innermost block.
    fn open_savepoint(&mut self) -> SavepointName {
        self.context.open_savepoint()
    }

    /// Ends the innermost block and returns the savepoint name it used.
    fn close_savepoint(&mut self) -> Option<SavepointName> {
        self.context.close_savepoint()
    }
}

/// A synchronous connection exposing the transaction bookkeeping of a scope.
///
/// The trait is the only adapter-specific part of a transaction block: the
/// connection stores a [`TransactionStatus`] and hands out a mutable reference
/// to it, while [`TransactionGuard`] owns the `BEGIN`, `SAVEPOINT`, and
/// rollback policy.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot open a RURQE transaction scope",
    label = "unsupported transactional connection",
    note = "synchronous adapters implement `TransactionalConnection` on top of `TransactionBackend`"
)]
pub trait TransactionalConnection: TransactionBackend {
    /// The transaction bookkeeping of the connection.
    fn transaction_status(&mut self) -> &mut TransactionStatus;
}

/// An open transaction or savepoint, rolled back unless it is committed.
///
/// The guard owns the borrow of the connection for the duration of a block, so
/// dropping it without [`commit`](TransactionGuard::commit) rolls the block
/// back: a failing or unwinding block never leaves a finished transaction or
/// savepoint behind. When the driver refuses that rollback the connection is
/// poisoned instead of being reported as clean.
pub struct TransactionGuard<'a, B: TransactionalConnection + ?Sized> {
    connection: &'a mut B,
    outer: bool,
    savepoint: Option<SavepointName>,
    finished: bool,
}

impl<'a, B: TransactionalConnection + ?Sized> TransactionGuard<'a, B> {
    /// Opens the transaction of `connection`, or a savepoint when one is open.
    ///
    /// A connection poisoned by a failed rollback never opens a new block
    /// silently: the abandoned transaction is rolled back first, so a driver that
    /// refuses the rollback reports the failure here instead of running the next
    /// block inside a transaction whose state is unknown.
    pub fn begin(connection: &'a mut B) -> Result<Self, B::Error> {
        Self::recover(connection)?;

        if connection.transaction_status().is_open() {
            let savepoint = connection.transaction_status().open_savepoint();
            match connection.savepoint(&savepoint) {
                Ok(()) => {
                    #[cfg(feature = "tracing")]
                    tracing::debug!(savepoint = %savepoint, "creating savepoint");
                    Ok(TransactionGuard {
                        connection,
                        outer: false,
                        savepoint: Some(savepoint),
                        finished: false,
                    })
                }
                Err(error) => {
                    connection.transaction_status().close_savepoint();
                    Err(error)
                }
            }
        } else {
            connection.begin(None)?;
            connection.transaction_status().mark_open(true);
            #[cfg(feature = "tracing")]
            tracing::info!("beginning transaction");
            Ok(TransactionGuard {
                connection,
                outer: true,
                savepoint: None,
                finished: false,
            })
        }
    }

    /// Closes the transaction a failed rollback left behind.
    ///
    /// The retry either finishes the abandoned rollback, which leaves the
    /// connection clean, or hands the driver error back to the caller, which is
    /// the signal that the connection has to be discarded rather than reused.
    fn recover(connection: &mut B) -> Result<(), B::Error> {
        if !connection.transaction_status().is_poisoned() {
            return Ok(());
        }

        match connection.rollback() {
            Ok(()) => {
                connection.transaction_status().mark_closed();
                #[cfg(feature = "tracing")]
                tracing::warn!("recovered a connection whose rollback had failed");
                Ok(())
            }
            Err(error) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    "rolling back the abandoned transaction failed again; the connection has to be discarded"
                );
                Err(error)
            }
        }
    }

    /// Whether the guard owns the outermost transaction block.
    pub fn is_outer(&self) -> bool {
        self.outer
    }

    /// The savepoint guarding a nested block, if the guard is nested.
    pub fn savepoint(&self) -> Option<&SavepointName> {
        self.savepoint.as_ref()
    }

    /// Commits the outermost transaction or releases the nested savepoint.
    pub fn commit(mut self) -> Result<(), B::Error> {
        self.release()?;
        self.finished = true;
        Ok(())
    }

    /// Commits or releases the guarded block.
    fn release(&mut self) -> Result<(), B::Error> {
        if self.outer {
            self.connection.commit()?;
            self.connection.transaction_status().mark_closed();
            #[cfg(feature = "tracing")]
            tracing::info!("committed transaction");
        } else if let Some(savepoint) = self.savepoint.as_ref() {
            self.connection.release_savepoint(savepoint)?;
            self.connection.transaction_status().close_savepoint();
            #[cfg(feature = "tracing")]
            tracing::debug!(savepoint = %savepoint, "released savepoint");
        }
        Ok(())
    }
}

impl<B: TransactionalConnection + ?Sized> Deref for TransactionGuard<'_, B> {
    type Target = B;

    fn deref(&self) -> &B {
        self.connection
    }
}

impl<B: TransactionalConnection + ?Sized> DerefMut for TransactionGuard<'_, B> {
    fn deref_mut(&mut self) -> &mut B {
        self.connection
    }
}

impl<B: TransactionalConnection + ?Sized> Drop for TransactionGuard<'_, B> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.outer {
            match self.connection.rollback() {
                Ok(()) => {
                    self.connection.transaction_status().mark_closed();
                    #[cfg(feature = "tracing")]
                    tracing::warn!("rolled back transaction");
                }
                Err(_) => {
                    // The work of the block may still be pending, so the
                    // bookkeeping keeps the transaction open and poisons the
                    // connection: the next `begin` retries this rollback instead
                    // of opening a fresh transaction on top of the abandoned one.
                    self.connection.transaction_status().mark_poisoned();
                    #[cfg(feature = "tracing")]
                    tracing::error!(
                        "rolling back the transaction failed; the connection stays poisoned until it is discarded"
                    );
                }
            }
        } else if let Some(savepoint) = self.savepoint.as_ref() {
            let closed = match self.connection.rollback_to_savepoint(savepoint) {
                Ok(()) => self.connection.release_savepoint(savepoint).is_ok(),
                Err(_) => false,
            };
            if closed {
                self.connection.transaction_status().close_savepoint();
                #[cfg(feature = "tracing")]
                tracing::warn!(savepoint = %savepoint, "rolled back to savepoint");
            } else {
                self.connection.transaction_status().mark_poisoned();
                #[cfg(feature = "tracing")]
                tracing::error!(
                    savepoint = %savepoint,
                    "rolling back to the savepoint failed; the connection stays poisoned until it is discarded"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::expr::SqlVal;
    use crate::query::{BuiltQuery, QueryBackend};

    /// A connection whose transactional statements are scripted by the test.
    #[derive(Default)]
    struct MockConnection {
        status: TransactionStatus,
        statements: Vec<String>,
        fail_rollback: bool,
        fail_savepoint_rollback: bool,
    }

    impl QueryBackend for MockConnection {
        type Error = String;

        fn fetch_raw(&mut self, _query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            Ok(Vec::new())
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            self.statements.push(query.sql.clone());
            Ok(0)
        }
    }

    impl TransactionBackend for MockConnection {
        fn rollback(&mut self) -> Result<(), Self::Error> {
            self.statements.push(String::from("ROLLBACK"));
            if self.fail_rollback {
                Err(String::from("rollback refused"))
            } else {
                Ok(())
            }
        }

        fn rollback_to_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
            self.statements.push(format!("ROLLBACK TO SAVEPOINT {name}"));
            if self.fail_savepoint_rollback {
                Err(String::from("savepoint rollback refused"))
            } else {
                Ok(())
            }
        }
    }

    impl TransactionalConnection for MockConnection {
        fn transaction_status(&mut self) -> &mut TransactionStatus {
            &mut self.status
        }
    }

    #[test]
    fn a_failed_rollback_poisons_the_connection() {
        let mut connection = MockConnection {
            fail_rollback: true,
            ..MockConnection::default()
        };

        drop(TransactionGuard::begin(&mut connection).expect("the transaction opens"));

        assert!(connection.status.is_poisoned());
        assert!(
            connection.status.is_open(),
            "a refused rollback never closes the transaction"
        );
        assert_eq!(
            connection.statements,
            vec![String::from("BEGIN"), String::from("ROLLBACK")]
        );
    }

    #[test]
    fn a_poisoned_connection_refuses_a_new_transaction() {
        let mut connection = MockConnection {
            fail_rollback: true,
            ..MockConnection::default()
        };
        drop(TransactionGuard::begin(&mut connection).expect("the transaction opens"));

        let refused = TransactionGuard::begin(&mut connection);

        assert_eq!(refused.err(), Some(String::from("rollback refused")));
        assert!(connection.status.is_poisoned());
        assert!(connection.status.is_open());
    }

    #[test]
    fn a_poisoned_connection_recovers_once_the_rollback_succeeds() {
        let mut connection = MockConnection {
            fail_rollback: true,
            ..MockConnection::default()
        };
        drop(TransactionGuard::begin(&mut connection).expect("the transaction opens"));
        assert!(connection.status.is_poisoned());

        connection.fail_rollback = false;
        let guard = TransactionGuard::begin(&mut connection).expect("the connection recovers");
        assert!(guard.is_outer());
        assert!(!guard.status.is_poisoned());
        assert!(guard.status.is_open());
        guard.commit().expect("the recovered transaction commits");

        assert!(!connection.status.is_open());
        assert!(!connection.status.is_poisoned());
    }

    #[test]
    fn a_failed_savepoint_rollback_poisons_the_connection() {
        let mut connection = MockConnection {
            fail_savepoint_rollback: true,
            ..MockConnection::default()
        };
        let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
        drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));

        assert!(outer.status.is_poisoned());
        assert!(
            outer.status.is_open(),
            "the enclosing transaction stays open"
        );
        assert_eq!(
            outer.status.depth(),
            1,
            "a refused savepoint rollback keeps the savepoint recorded"
        );
    }

    #[test]
    fn a_nested_block_rolls_back_to_its_savepoint() {
        let mut connection = MockConnection::default();
        let mut outer = TransactionGuard::begin(&mut connection).expect("the transaction opens");
        drop(TransactionGuard::begin(&mut *outer).expect("the savepoint opens"));

        assert!(!outer.status.is_poisoned());
        assert!(outer.status.is_open());
        assert_eq!(outer.status.depth(), 0);

        outer.commit().expect("the outer transaction commits");
        assert!(!connection.status.is_open());
    }
}
