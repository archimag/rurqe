//! The guard driving one synchronous transaction block.
//!
//! A [`TransactionGuard`] decides whether a call opens a transaction or nests a
//! savepoint inside the one already open, commits the outermost transaction or
//! releases the nested savepoint, and rolls a failing or unwinding block back.
//! Whether a refused rollback poisons the connection is the adapter's
//! classification rather than the guard's guess: the guard only records it.

use core::ops::{Deref, DerefMut};

use crate::query::savepoint::SavepointName;

use super::backend::{IsolationLevel, TransactionalConnection};

/// A synchronous connection exposing the transaction bookkeeping of a scope.
///
/// The guard owns the borrow of the connection for the duration of a block, so
/// dropping it without [`commit`](TransactionGuard::commit) rolls the block
/// back: a failing or unwinding block never leaves a finished transaction or
/// savepoint behind. When the driver refuses that rollback the connection is
/// poisoned instead of being reported as clean, and the guard holding the
/// abandoned transaction refuses to commit it, so the work that rollback was
/// supposed to discard is never persisted by an enclosing block.
pub struct TransactionGuard<'a, B: TransactionalConnection + ?Sized> {
    connection: &'a mut B,
    outer: bool,
    savepoint: Option<SavepointName>,
    finished: bool,
}

impl<'a, B: TransactionalConnection + ?Sized> TransactionGuard<'a, B> {
    /// Opens the transaction of `connection` with the backend default isolation
    /// level, or a savepoint when one is already open.
    ///
    /// The method is [`begin_with`](TransactionGuard::begin_with) without an
    /// isolation level, so the rendered `BEGIN` carries no `ISOLATION LEVEL`
    /// clause.
    ///
    /// A connection poisoned by a failed rollback never opens a new block
    /// silently: the abandoned work is recovered first, so a driver that refuses
    /// the recovery reports the failure here instead of running the next block
    /// inside a transaction whose state is unknown. What the recovery rolls back
    /// depends on ownership: without a live owner the whole transaction is
    /// rolled back, while a transaction a live guard still owns only has its
    /// abandoned savepoint closed, so a nested block can never discard the
    /// enclosing transaction.
    pub fn begin(connection: &'a mut B) -> Result<Self, B::Error> {
        Self::begin_with(connection, None)
    }

    /// Opens the transaction of `connection` with `isolation`, or a savepoint
    /// when a transaction is already open.
    ///
    /// The level is rendered into the `BEGIN` statement as
    /// `BEGIN ISOLATION LEVEL <level>` through [`IsolationLevel::as_str`], and
    /// `None` keeps the statement at the backend default. The level applies only
    /// to the outermost transaction: a call made while a transaction is already
    /// open nests inside a `SAVEPOINT` statement carrying no isolation clause,
    /// and because SQL defines no per-savepoint isolation the requested level is
    /// ignored for such a block while the enclosing transaction keeps its own.
    ///
    /// A connection poisoned by a failed rollback never opens a new block
    /// silently: the abandoned work is recovered first, so a driver that refuses
    /// the recovery reports the failure here instead of running the next block
    /// inside a transaction whose state is unknown. What the recovery rolls back
    /// depends on ownership: without a live owner the whole transaction is
    /// rolled back, while a transaction a live guard still owns only has its
    /// abandoned savepoint closed, so a nested block can never discard the
    /// enclosing transaction.
    pub fn begin_with(
        connection: &'a mut B,
        isolation: Option<IsolationLevel>,
    ) -> Result<Self, B::Error> {
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
            connection.begin(isolation)?;
            connection.transaction_status().mark_open(true);
            connection.transaction_status().mark_owner(true);
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

    /// Recovers the abandoned work a failed rollback left behind.
    ///
    /// The action depends on ownership, and on nothing else. A transaction no
    /// live guard owns is rolled back as a whole, which is the only way to
    /// discard the pending work of a guard that is gone; a driver that refuses
    /// that rollback hands its error back, which is the signal that the
    /// connection has to be discarded rather than reused.
    ///
    /// A transaction a live guard still owns is the other case: the pending
    /// failure belongs to a savepoint of that transaction, because a failing
    /// nested drop keeps its savepoint recorded. Only the abandoned savepoint
    /// closure is retried, so the enclosing transaction and the work the
    /// enclosing guard is responsible for stay intact. A retry that heals
    /// clears the poison and lets the caller open the fresh savepoint, while a
    /// retry that fails again keeps the pending poison, so the enclosing guard
    /// refuses to commit and reports the failure instead of silently persisting
    /// work that was supposed to be rolled back.
    ///
    /// A poisoned transaction recorded as owned but carrying no savepoint cannot
    /// be reached through a guard, because a failing nested drop keeps its
    /// savepoint; the case falls back to the full rollback rather than leaving
    /// the connection unrecovered.
    fn recover(connection: &mut B) -> Result<(), B::Error> {
        if !connection.transaction_status().is_poisoned() {
            return Ok(());
        }

        let owned = connection.transaction_status().is_owned();
        let abandoned = if owned {
            // The name is read back from the recorded savepoint, which was
            // validated on the way in, so rebuilding it cannot fail in practice.
            connection
                .transaction_status()
                .current_savepoint()
                .and_then(|name| SavepointName::new(name.to_owned()).ok())
        } else {
            None
        };

        if let Some(savepoint) = abandoned {
            let failure = match connection.rollback_to_savepoint(&savepoint) {
                Ok(()) => connection.release_savepoint(&savepoint).err(),
                Err(error) => Some(error),
            };

            return match failure {
                None => {
                    let _ = connection.transaction_status().take_poison();
                    connection.transaction_status().close_savepoint();
                    #[cfg(feature = "tracing")]
                    tracing::warn!(
                        savepoint = %savepoint,
                        "recovered the abandoned savepoint of a live transaction"
                    );
                    Ok(())
                }
                Some(error) => {
                    // The pending poison stays recorded rather than being
                    // replaced by this failure: it is the signal that makes the
                    // enclosing guard refuse to commit, and hand-holding the
                    // only copy of it back to this caller would lose it.
                    #[cfg(feature = "tracing")]
                    tracing::error!(
                        savepoint = %savepoint,
                        "rolling back to the abandoned savepoint failed again; the enclosing transaction keeps its pending failure"
                    );
                    Err(error)
                }
            };
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
    ///
    /// A connection poisoned by a failed rollback never commits: the work of the
    /// block whose rollback the driver refused may still be pending inside the
    /// open transaction, so the method hands that driver error back instead of
    /// persisting it, and the guard stays unfinished so that dropping it forces
    /// the rollback the connection needs.
    pub fn commit(mut self) -> Result<(), B::Error> {
        self.release()?;
        self.finished = true;
        Ok(())
    }

    /// Commits or releases the guarded block.
    ///
    /// The poison check runs before either branch, so an enclosing guard refuses
    /// to commit the transaction and a nested one refuses to release the
    /// savepoint as long as the work of a block whose rollback failed is still
    /// pending.
    fn release(&mut self) -> Result<(), B::Error> {
        if let Some(error) = self.connection.transaction_status().take_poison() {
            #[cfg(feature = "tracing")]
            tracing::error!(
                "refusing to commit a transaction whose rollback failed; the connection has to be discarded"
            );
            return Err(error);
        }

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
                Err(error) => {
                    // The work of the block may still be pending, so the
                    // bookkeeping keeps the transaction open and poisons the
                    // connection: the next `begin` retries this rollback instead
                    // of opening a fresh transaction on top of the abandoned one,
                    // and committing this transaction is refused with the error
                    // the driver reported here.
                    self.connection.transaction_status().mark_poisoned(error);
                    // The dying guard abandons its transaction, so the next
                    // `begin` on the bare connection performs the full recovery
                    // instead of retrying the savepoint of a transaction nobody
                    // owns any more.
                    self.connection.transaction_status().mark_owner(false);
                    #[cfg(feature = "tracing")]
                    tracing::error!(
                        "rolling back the transaction failed; the connection stays poisoned until it is discarded"
                    );
                }
            }
        } else if let Some(savepoint) = self.savepoint.as_ref() {
            let failure = match self.connection.rollback_to_savepoint(savepoint) {
                Ok(()) => self.connection.release_savepoint(savepoint).err(),
                Err(error) => Some(error),
            };
            match failure {
                None => {
                    self.connection.transaction_status().close_savepoint();
                    #[cfg(feature = "tracing")]
                    tracing::warn!(savepoint = %savepoint, "rolled back to savepoint");
                }
                Some(error) => {
                    self.connection.transaction_status().mark_poisoned(error);
                    #[cfg(feature = "tracing")]
                    tracing::error!(
                        savepoint = %savepoint,
                        "rolling back to the savepoint failed; the connection stays poisoned until it is discarded"
                    );
                }
            }
        }
    }
}
