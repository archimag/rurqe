//! The savepoint stack and the transaction bookkeeping of a synchronous connection.
//!
//! [`TransactionContext`] tracks the savepoint stack of a nested block, and
//! [`TransactionStatus`] wraps that stack with the open, owner, and poison
//! bookkeeping of the outermost transaction. The two live together because the
//! guard that drives a block mutates one structure, and every savepoint it opens
//! is accounted for by the transaction that holds it.

use crate::query::savepoint::SavepointName;

/// Tracks the nesting depth and savepoint names of an open transaction.
#[derive(Debug, Clone, Default)]
pub struct TransactionContext {
    next_savepoint: u32,
    savepoints: Vec<SavepointName>,
}

impl TransactionContext {
    /// Creates a context for the outermost transaction.
    pub fn new() -> Self {
        TransactionContext::default()
    }

    /// The number of nested savepoints currently open.
    pub fn depth(&self) -> usize {
        self.savepoints.len()
    }

    /// Whether the context currently sits inside a nested transaction.
    pub fn is_nested(&self) -> bool {
        !self.savepoints.is_empty()
    }

    /// The name of the innermost open savepoint, if any.
    pub fn current_savepoint(&self) -> Option<&str> {
        self.savepoints.last().map(SavepointName::as_str)
    }

    /// Allocates a fresh savepoint name and makes it the innermost block.
    pub fn open_savepoint(&mut self) -> SavepointName {
        self.next_savepoint += 1;
        let name = SavepointName::generated(self.next_savepoint);
        self.savepoints.push(name.clone());
        name
    }

    /// Ends the innermost block and returns the savepoint name it used.
    pub fn close_savepoint(&mut self) -> Option<SavepointName> {
        self.savepoints.pop()
    }
}

/// The transaction bookkeeping a synchronous connection keeps between calls.
///
/// The status is parameterised by the driver error of the connection, because a
/// rollback the driver refused is retried later: the error that poisoned the
/// connection stays recorded until the abandoned transaction is closed, and the
/// guard that refuses to commit it hands that error back to the caller.
#[derive(Debug, Clone, Default)]
pub struct TransactionStatus<E = ()> {
    context: TransactionContext,
    open: bool,
    poison: Option<E>,
    owned: bool,
}

impl<E> TransactionStatus<E> {
    /// Creates the bookkeeping of a connection without an open transaction.
    pub fn new() -> Self {
        TransactionStatus {
            context: TransactionContext::new(),
            open: false,
            poison: None,
            owned: false,
        }
    }

    /// Whether the outermost transaction block is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether a failed rollback left the connection in an unknown state.
    ///
    ///
    /// A transaction the driver refused to roll back may still hold the work of
    /// its block, so the connection stays poisoned until that transaction is
    /// closed for real: until then the guard refuses both to open a new
    /// transaction and to commit the one that holds the pending work.
    pub fn is_poisoned(&self) -> bool {
        self.poison.is_some()
    }

    /// Whether a live guard owns the outermost transaction block.
    ///
    /// Ownership is what tells one poison apart from the other: a failure left
    /// by a nested guard poisons a transaction that a live guard still owns, so
    /// the pending work can be retried inside that transaction, while a failure
    /// left by an outermost guard belongs to a transaction nobody owns any more
    /// and has to be rolled back as a whole.
    pub fn is_owned(&self) -> bool {
        self.owned
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
    pub(super) fn mark_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Records whether a live guard owns the outermost transaction block.
    pub(super) fn mark_owner(&mut self, owned: bool) {
        self.owned = owned;
    }

    /// Records the driver error of a refused rollback, poisoning the connection.
    pub(super) fn mark_poisoned(&mut self, error: E) {
        self.poison = Some(error);
    }

    /// Takes the driver error of the refused rollback that poisoned the connection.
    ///
    /// The guard takes the error when it refuses to commit, so the failure the
    /// driver reported reaches the caller instead of the block being committed
    /// with the work the rollback was supposed to discard.
    pub(super) fn take_poison(&mut self) -> Option<E> {
        self.poison.take()
    }

    /// Records that the connection left the outermost transaction block.
    ///
    /// Closing the outermost transaction discards its savepoints on the server,
    /// so the bookkeeping drops them as well instead of keeping names that the
    /// connection never opened in the first place.
    pub(super) fn mark_closed(&mut self) {
        self.open = false;
        self.owned = false;
        self.poison = None;
        while self.context.close_savepoint().is_some() {}
    }

    /// Allocates a fresh savepoint name and makes it the innermost block.
    pub(super) fn open_savepoint(&mut self) -> SavepointName {
        self.context.open_savepoint()
    }

    /// Ends the innermost block and returns the savepoint name it used.
    pub(super) fn close_savepoint(&mut self) -> Option<SavepointName> {
        self.context.close_savepoint()
    }
}
