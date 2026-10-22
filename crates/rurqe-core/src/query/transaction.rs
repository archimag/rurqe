//! Nestable transaction scopes shared by the synchronous driver adapters.
//!
//! Every synchronous adapter offers the same contract: the outermost
//! `transaction` call runs its block between `BEGIN` and `COMMIT`, a call made
//! while a transaction is already open wraps its block in a `SAVEPOINT`, and a
//! failing or unwinding block is rolled back. [`TransactionGuard`] implements
//! that contract once on top of [`TransactionBackend`] and
//! [`TransactionStatus`], so an adapter only exposes its bookkeeping and never
//! repeats the guard.

use core::ops::{Deref, DerefMut};

use super::execution::{TransactionBackend, TransactionContext};
use super::savepoint::SavepointName;

/// The transaction bookkeeping a synchronous connection keeps between calls.
#[derive(Debug, Clone, Default)]
pub struct TransactionStatus {
    context: TransactionContext,
    open: bool,
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
/// back: a failing or unwinding block never leaves an open transaction or
/// savepoint behind.
pub struct TransactionGuard<'a, B: TransactionalConnection + ?Sized> {
    connection: &'a mut B,
    outer: bool,
    savepoint: Option<SavepointName>,
    finished: bool,
}

impl<'a, B: TransactionalConnection + ?Sized> TransactionGuard<'a, B> {
    /// Opens the transaction of `connection`, or a savepoint when one is open.
    pub fn begin(connection: &'a mut B) -> Result<Self, B::Error> {
        if connection.transaction_status().is_open() {
            let savepoint = connection.transaction_status().open_savepoint();
            match connection.savepoint(&savepoint) {
                Ok(()) => {
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
            tracing::info!("beginning transaction");
            Ok(TransactionGuard {
                connection,
                outer: true,
                savepoint: None,
                finished: false,
            })
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
            self.connection.transaction_status().mark_open(false);
            tracing::info!("committed transaction");
        } else if let Some(savepoint) = self.savepoint.as_ref() {
            self.connection.release_savepoint(savepoint)?;
            self.connection.transaction_status().close_savepoint();
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
            let _ = self.connection.rollback();
            self.connection.transaction_status().mark_open(false);
            tracing::warn!("rolled back transaction");
        } else if let Some(savepoint) = self.savepoint.as_ref() {
            let _ = self.connection.rollback_to_savepoint(savepoint);
            let _ = self.connection.release_savepoint(savepoint);
            self.connection.transaction_status().close_savepoint();
            tracing::warn!(savepoint = %savepoint, "rolled back to savepoint");
        }
    }
}
