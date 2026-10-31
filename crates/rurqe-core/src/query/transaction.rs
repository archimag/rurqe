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
//! reports such a block as closed: it poisons the connection instead, the guard
//! holding the abandoned transaction refuses to commit it rather than persisting
//! the work the rollback was supposed to discard, and the next
//! [`TransactionGuard::begin`] recovers the abandoned work before it opens
//! anything. What the recovery may touch is decided by ownership rather than by
//! depth: a poison left by an outermost guard, which is gone, is recovered by
//! rolling the whole transaction back, while a poison left by a nested guard
//! whose enclosing transaction is still owned by a live guard is recovered by
//! closing only the abandoned savepoint. A nested `begin` therefore never
//! discards a transaction an enclosing guard still owns, and the enclosing
//! `commit()` reports the pending failure instead of silently succeeding.
//!
//! Whether a refused rollback means an abandoned transaction or a block that a
//! raw driver handle already closed is a driver observation rather than a fact
//! the guard can derive: a caller bypassing the guard issues its own `COMMIT` or
//! `ROLLBACK` through the adapter's raw handle, and the guard only observes that
//! its next rollback was refused. The classification therefore belongs to the
//! adapter, which overrides [`TransactionBackend::rollback`] and answers a
//! refusal the driver reports as "no transaction is open" with `Ok`. That answer
//! is what degrades a desync into a no-op that leaves the connection clean:
//! `Drop` and the recovery path of [`TransactionGuard::begin`] record the
//! connection clean instead of poisoning it, so a healthy adapter accepts the
//! next typed transaction.
//!
//! The isolation level belongs to the outermost transaction only:
//! [`TransactionGuard::begin_with`] renders `BEGIN ISOLATION LEVEL ...`, while
//! [`TransactionGuard::begin`] keeps the backend default, and a guard opened
//! inside an open transaction becomes a savepoint, which SQL gives no isolation
//! of its own.

mod backend;
mod guard;
mod status;

pub use self::backend::{IsolationLevel, TransactionBackend, TransactionalConnection};
pub use self::guard::TransactionGuard;
pub use self::status::{TransactionContext, TransactionStatus};

// The protocol test module reaches the savepoint identifier through its
// `use super::*` glob, while nothing in this module names it directly.
#[cfg(test)]
use super::savepoint::SavepointName;

#[cfg(test)]
mod tests;
