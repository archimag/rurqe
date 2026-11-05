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
//!
//! The record keeps the failure behind the state rather than only its
//! description: a refusal constructed from it renders the reason and hands the
//! driver failure back through [`source`](std::error::Error::source), so a
//! caller inspects the failure programmatically instead of parsing the message.
//! The failure is retained exactly where the adapter traces it instead of
//! returning it — the savepoint rollback of a nested block, the rollback a
//! connection issues once its closure returned, and the rollback a pooled block
//! issues for a failed block. A path that hands the driver failure to its own
//! caller records the rendered reason alone, because that caller holds the
//! failure itself.

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

/// Reason rendered by a refusal built from a cell that recorded nothing.
///
/// Every refusal is produced while the cell is poisoned, so the state is not
/// reachable through the adapters; rendering it keeps the refusal free of the
/// unwrapping a panic-free adapter cannot perform.
pub(crate) const NO_RECORDED_REASON: &str = "the connection recorded no reason";

/// Interior-mutably shared poison record of one connection.
///
/// The flag and the record are kept apart so that reading whether a connection
/// is poisoned costs one atomic load, while the record that explains the state
/// is read only when a statement is refused.
#[derive(Debug)]
pub(crate) struct PoisonCell {
    /// Whether a poison reason has been recorded.
    poisoned: AtomicBool,
    /// First recorded failure, held behind a mutex because it is written once
    /// and read rarely.
    record: Mutex<Option<PoisonRecord>>,
}

/// The failure that poisoned a connection, kept so a refusal can report it.
///
/// The rendered reason is what a diagnostic reads, while the driver failure is
/// what a caller inspects: it carries the variant, the SQLSTATE, and the cause
/// chain the engine reported. The failure is retained only where the adapter
/// traces it instead of returning it, because a path that returns the driver
/// failure already hands its caller the value this record would duplicate.
#[derive(Debug)]
struct PoisonRecord {
    /// Rendered description of the failure that produced the state.
    reason: String,
    /// Driver failure the adapter traced instead of returning it to a caller.
    cause: Option<Arc<PgError>>,
}

impl PoisonCell {
    /// Creates a cell that records no poison.
    pub(crate) fn new() -> Self {
        Self {
            poisoned: AtomicBool::new(false),
            record: Mutex::new(None),
        }
    }

    /// Records `reason` and marks the cell poisoned.
    ///
    /// The mark belongs to the paths that hand the driver failure to their own
    /// caller: the caller already holds the failure, so the cell keeps the
    /// rendered reason alone instead of a second copy of it. A path that traces
    /// the failure records it together with the failure itself through
    /// [`mark_with_cause`](PoisonCell::mark_with_cause).
    pub(crate) fn mark(&self, reason: String) {
        self.record(reason, None);
    }

    /// Records `reason` together with the driver failure behind it.
    ///
    /// The failure is what a later refusal hands back through
    /// [`source`](std::error::Error::source), so a path that traces the failure
    /// instead of returning it does not lose it.
    pub(crate) fn mark_with_cause(&self, reason: String, cause: PgError) {
        self.record(reason, Some(Arc::new(cause)));
    }

    /// Records `reason` and an optional `cause` and marks the cell poisoned.
    ///
    /// The record is sticky and the first one wins: a connection reaches the
    /// poisoned state once, and the failure that produced it is the one that
    /// explains the state. A later mark — the unwind guard of a block whose
    /// `COMMIT` was already refused, for instance — is a no-op, so the
    /// diagnostic a caller reads never depends on the order in which two
    /// failures were observed.
    fn record(&self, reason: String, cause: Option<Arc<PgError>>) {
        {
            let mut slot = self.record_slot();
            if slot.is_none() {
                *slot = Some(PoisonRecord { reason, cause });
            }
        }

        // The record is stored before the flag is raised, so a reader that
        // observes the poisoned state also observes the record explaining it.
        self.poisoned.store(true, Ordering::SeqCst);
    }

    /// Reports whether the cell records a poison.
    pub(crate) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }

    /// Returns the first recorded reason, when the cell is poisoned.
    pub(crate) fn poison_reason(&self) -> Option<String> {
        let slot = self.record_slot();
        slot.as_ref().map(|record| record.reason.clone())
    }

    /// Renders the refusal of the connection this cell belongs to.
    ///
    /// The refusal carries the recorded reason and, when the adapter traced the
    /// failure instead of returning it, that failure as well, so a caller
    /// inspects what poisoned the connection instead of only reading where the
    /// transaction state broke.
    pub(crate) fn refusal(&self) -> PgError {
        match self.record_slot().as_ref() {
            Some(record) => PgError::ConnectionPoisoned {
                reason: record.reason.clone(),
                cause: record.cause.clone(),
            },
            None => PgError::ConnectionPoisoned {
                reason: String::from(NO_RECORDED_REASON),
                cause: None,
            },
        }
    }

    /// Locks the record slot, recovering it from a holder that panicked.
    ///
    /// The mutex is held for the few instructions of a mark or a read, so a
    /// lock reported as poisoned means a thread panicked outside the critical
    /// section of this module. The slot stays structurally sound in that case,
    /// and the protocol reads it instead of failing, because the poison state
    /// has to stay observable through a panic.
    fn record_slot(&self) -> MutexGuard<'_, Option<PoisonRecord>> {
        match self.record.lock() {
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
        assert_eq!(
            cell.poison_reason().as_deref(),
            Some(super::ABANDONED_BLOCK)
        );
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
            cause: None,
        };

        assert_eq!(
            failure.to_string(),
            "the PostgreSQL connection is poisoned: `COMMIT` failed"
        );
        assert!(failure.source().is_none());
    }

    #[test]
    fn a_refusal_hands_the_recorded_driver_failure_back_through_its_source() {
        let cell = PoisonCell::new();
        let failure = PgError::InvalidValue {
            column_type: String::from("text"),
            reason: String::from("the value is not a text value"),
        };
        cell.mark_with_cause(control_failure("ROLLBACK TO SAVEPOINT", &failure), failure);

        let refusal = cell.refusal();
        assert_eq!(
            refusal.to_string(),
            "the PostgreSQL connection is poisoned: `ROLLBACK TO SAVEPOINT` failed: \
             invalid value for PostgreSQL column type `text`: the value is not a text value"
        );

        let cause = refusal
            .source()
            .expect("the refusal hands back the failure behind it");
        assert!(
            cause.is::<PgError>(),
            "the source is the driver failure the adapter traced"
        );
    }

    #[test]
    fn a_refusal_of_a_returned_failure_reports_no_source() {
        let cell = PoisonCell::new();
        cell.mark(String::from("`BEGIN` failed: the connection is closed"));

        let refusal = cell.refusal();
        assert_eq!(
            refusal.to_string(),
            "the PostgreSQL connection is poisoned: `BEGIN` failed: the connection is closed"
        );
        assert!(
            refusal.source().is_none(),
            "the caller that records the reason already holds the failure"
        );
    }
}
