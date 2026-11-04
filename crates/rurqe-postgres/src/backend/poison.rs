//! Poison protocol shared by the PostgreSQL adapters.
//!
//! A connection is *poisoned* once the adapter can no longer prove that it is
//! outside a transaction. Three endings produce that state: a transaction-control
//! statement the driver refused, a block that panicked, and a block whose future
//! was dropped before its control statement ran. PostgreSQL accepts a `BEGIN`
//! inside an open transaction with a warning instead of refusing it, so a
//! connection in that state would execute the statements of the next caller
//! inside the abandoned transaction and persist them with its own `COMMIT`.
//!
//! The record is shared through an [`Arc`](std::sync::Arc) by the connection and
//! every handle it creates, and it is sticky: a poisoned connection refuses
//! further work with [`PgError::ConnectionPoisoned`] until its owner discards it.
//! The adapter never clears the state, because nothing it could run would close
//! the abandoned transaction with certainty, and the caller that owns the handle
//! is the only party able to discard it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::types::PgError;

/// Reason recorded when a guarded block ended without closing its transaction.
pub(crate) const ABANDONED_BLOCK: &str =
    "the transaction block ended without a clean COMMIT or ROLLBACK";

/// Reason recorded when a statement checkout ended before its statement settled.
///
/// A statement owns no control statement, so a checkout that ends before its
/// statement settled destroys the connection for a reason of its own instead of
/// borrowing the transaction-block one.
pub(crate) const ABANDONED_STATEMENT: &str =
    "the statement checkout ended before its statement settled";

/// Describes a refused transaction-control statement as a poison reason.
///
/// The reason names the statement the adapter issued and carries the failure the
/// driver reported, so the diagnostic a later refusal reads states what broke the
/// transaction state instead of restating that it is broken.
pub(crate) fn control_failure(statement: &str, failure: &PgError) -> String {
    format!("`{statement}` failed: {failure}")
}

/// Interior-mutably shared poison record of one connection.
///
/// The flag and the reason are kept apart so that reading whether a connection is
/// poisoned costs one atomic load, while the diagnostic that explains the state
/// is read only when a statement is refused.
#[derive(Debug)]
pub(crate) struct PoisonCell {
    /// Whether a poison reason has been recorded.
    poisoned: AtomicBool,
    /// First recorded reason, held behind a mutex because it is written once and
    /// read rarely.
    reason: Mutex<Option<String>>,
}

impl PoisonCell {
    /// Creates a cell that records no poison.
    pub(crate) fn new() -> Self {
        Self {
            poisoned: AtomicBool::new(false),
            reason: Mutex::new(None),
        }
    }

    /// Records `reason` and marks the cell poisoned.
    ///
    /// The record is sticky and the first reason wins: a connection reaches the
    /// poisoned state once, and the failure that produced it is the one that
    /// explains the state. A later mark — the unwind guard of a block whose
    /// `COMMIT` was already refused, for instance — is a no-op, so the diagnostic
    /// a caller reads never depends on the order in which two failures were
    /// observed.
    pub(crate) fn mark(&self, reason: String) {
        {
            let mut slot = self.reason_slot();
            if slot.is_none() {
                *slot = Some(reason);
            }
        }

        // The reason is stored before the flag is raised, so a reader that
        // observes the poisoned state also observes the reason explaining it.
        self.poisoned.store(true, Ordering::SeqCst);
    }

    /// Reports whether the cell records a poison.
    pub(crate) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }

    /// Returns the first recorded reason, when the cell is poisoned.
    pub(crate) fn poison_reason(&self) -> Option<String> {
        let slot = self.reason_slot();
        (*slot).clone()
    }

    /// Locks the reason slot, recovering it from a holder that panicked.
    ///
    /// The mutex is held for the few instructions of a mark or a read, so a lock
    /// reported as poisoned means a thread panicked outside the critical section
    /// of this module. The slot stays structurally sound in that case, and the
    /// protocol reads it instead of failing, because the poison state has to stay
    /// observable through a panic.
    fn reason_slot(&self) -> MutexGuard<'_, Option<String>> {
        match self.reason.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Poisons its cell when the guarded block ends without a recorded clean finish.
///
/// The guard is created before the block opens, so the block never has to
/// remember an exit path: the `COMMIT` or `ROLLBACK` the adapter awaited and
/// accepted is recorded with [`mark_completed`](BlockExitGuard::mark_completed),
/// and every other ending — a panicking closure, a dropped future, a control
/// statement the driver refused — poisons the connection when the guard drops.
#[derive(Debug)]
pub(crate) struct BlockExitGuard {
    /// Poison record of the connection the block runs on.
    cell: Arc<PoisonCell>,
    /// Whether the block closed its transaction with an awaited statement.
    completed: bool,
}

impl BlockExitGuard {
    /// Guards `cell` for the block that is about to open.
    pub(crate) fn new(cell: Arc<PoisonCell>) -> Self {
        Self {
            cell,
            completed: false,
        }
    }

    /// Records that the block closed its transaction.
    ///
    /// A guard marked completed leaves its cell untouched when it drops, which is
    /// what distinguishes a block that committed or rolled back from one that was
    /// abandoned.
    pub(crate) fn mark_completed(&mut self) {
        self.completed = true;
    }
}

impl Drop for BlockExitGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.cell.mark(String::from(ABANDONED_BLOCK));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::sync::Arc;

    use super::{control_failure, BlockExitGuard, PoisonCell};
    use crate::PgError;

    #[test]
    fn a_fresh_cell_records_no_poison() {
        let cell = PoisonCell::new();

        assert!(!cell.is_poisoned());
        assert_eq!(cell.poison_reason(), None);
    }

    #[test]
    fn the_first_reason_wins_and_later_marks_are_ignored() {
        let cell = PoisonCell::new();
        cell.mark(String::from("`COMMIT` failed"));
        cell.mark(String::from("`ROLLBACK` failed"));

        assert!(cell.is_poisoned());
        assert_eq!(cell.poison_reason().as_deref(), Some("`COMMIT` failed"));
    }

    #[test]
    fn a_guard_poisons_its_cell_when_the_block_does_not_complete() {
        let cell = Arc::new(PoisonCell::new());
        {
            let _guard = BlockExitGuard::new(Arc::clone(&cell));
        }

        assert!(cell.is_poisoned());
        assert_eq!(cell.poison_reason().as_deref(), Some(super::ABANDONED_BLOCK));
    }

    #[test]
    fn a_completed_guard_leaves_its_cell_clean() {
        let cell = Arc::new(PoisonCell::new());
        {
            let mut guard = BlockExitGuard::new(Arc::clone(&cell));
            guard.mark_completed();
        }

        assert!(!cell.is_poisoned());
    }

    #[test]
    fn a_reason_names_the_statement_that_failed() {
        let failure = PgError::InvalidValue {
            column_type: String::from("text"),
            reason: String::from("the value is not a text value"),
        };

        assert_eq!(
            control_failure("COMMIT", &failure),
            "`COMMIT` failed: invalid value for PostgreSQL column type `text`: \
             the value is not a text value"
        );
    }

    #[test]
    fn a_poisoned_connection_renders_its_reason() {
        let failure = PgError::ConnectionPoisoned {
            reason: String::from("`COMMIT` failed"),
        };

        assert_eq!(
            failure.to_string(),
            "the PostgreSQL connection is poisoned: `COMMIT` failed"
        );
        assert!(failure.source().is_none());
    }
}
