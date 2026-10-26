//! The legal typestate transitions of the `SELECT` builder.
//!
//! Every builder records a clause and then restates the phantom parameters of the
//! statement it hands back. The transitions are enumerated here, one per state a
//! builder can legitimately reach, and each of them names exactly the parameter its
//! clause changed: the dialect is never reinterpreted, the row type, the scope, and
//! the lock state a caller did not ask about stay fixed by the signature, and the
//! source state pins the transition to the states the builder can arrive from. An
//! arbitrary reinterpretation of the whole statement is therefore not expressible:
//! `with_scope` restates the scope alone, `locked` claims the rows of an unlocked
//! statement alone, and `tail_closed` continues the ordering tail of a compound
//! statement alone.

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::query::scope::{Compound, TableScope};

use super::{
    HasProjection, LockUnavailable, LockWaitSet, Locked, NoLock, ProjectionOpen, SelectQuery,
    SetOpApplied, SetOperand, TailClosed,
};

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
    /// Reads the recorded statement under the relational scope `S`.
    ///
    /// Introducing a relation, joining one, or replacing the scope of a statement
    /// changes which relations its clauses address. Everything else the statement
    /// records survives the transition: the dialect, the projected row type, the
    /// projection state, and the lock state stay exactly as the builder recorded
    /// them, so a join cannot claim a row type or a lock it never established.
    pub(super) fn with_scope<S: TableScope>(self) -> SelectQuery<D, S, Output, Projected, Lock> {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }

    /// Fixes the row type of the statement to the rows its projection produces.
    ///
    /// The projection is recorded into the clause list of the statement first, so the
    /// transition only restates what those rows are: the dialect, the scope, and the
    /// lock state stay untouched. The `ProjectionOpen` bound keeps the transition to
    /// the states where a projection may still be recorded, so the rows a compound
    /// statement fixed cannot be rebound through it.
    pub(super) fn with_projection<Row>(self) -> SelectQuery<D, Tables, Row, HasProjection, Lock>
    where
        Lock: ProjectionOpen,
    {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected, NoLock> {
    /// Records that the statement can no longer request a row-level lock.
    ///
    /// Deduplicating the rows of a statement — with `DISTINCT` or on a column
    /// prefix — leaves one row per group, while a row-level lock claims the rows the
    /// statement read, so PostgreSQL refuses the pair. The deduplicating builders
    /// therefore move the query out of `NoLock` here, keeping the dialect, the
    /// scope, the row type, and the projection exactly as they were.
    pub(super) fn lock_unavailable(
        self,
    ) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable> {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables, Output> SelectQuery<D, Tables, Output, HasProjection, NoLock> {
    /// Records that the statement claims the rows it selected for its transaction.
    ///
    /// The locking builders request the clause and then move the query here, so the
    /// closing wait modifier is reachable only from the state a real locking clause
    /// established. The dialect, the scope, and the projected rows stay untouched.
    pub(super) fn locked(self) -> SelectQuery<D, Tables, Output, HasProjection, Locked> {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables, Output> SelectQuery<D, Tables, Output, HasProjection, Locked> {
    /// Records the wait behaviour a locking clause fixed.
    ///
    /// SQL refuses `NOWAIT` together with `SKIP LOCKED`, so the modifier moves the
    /// query here, where neither of them is reachable again.
    pub(super) fn lock_wait_set(self) -> SelectQuery<D, Tables, Output, HasProjection, LockWaitSet> {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables, Output, Lock: SetOperand>
    SelectQuery<D, Tables, Output, HasProjection, Lock>
{
    /// Records that a set operation combined the statement with another operand.
    ///
    /// The combined statement produces the output columns of its first projection
    /// list and can neither keep the lock of an operand nor accept a new one, so the
    /// transition restates the scope as `Compound` and the lock state as
    /// `SetOpApplied` at once. The dialect and the output row type stay untouched.
    pub(super) fn combined(self) -> SelectQuery<D, Compound, Output, HasProjection, SetOpApplied> {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

/// Implements the transition closing the ordering tail for one compound state.
///
/// The ordering of a compound statement belongs to the whole result: the first
/// criterion closes the statement for further operands, and every criterion after
/// it extends that same clause. Both compound states therefore need the identical
/// transition, while a plain statement carries no compound ordering at all. The
/// macro spells it once per state instead of widening the impl header to a trait
/// bound, so the state the transition may be reached from stays an explicit part
/// of the header — and the helper trait that would otherwise carry it does not
/// leak into the visibility of the generated impl.
macro_rules! impl_tail_closed {
    ($lock:ty) => {
        impl<D: Dialect, Output> SelectQuery<D, Compound, Output, HasProjection, $lock> {
            /// Records another ordering criterion of the combined rows.
            ///
            /// Every criterion of the compound statement leaves it in
            /// [`TailClosed`], where another operand can no longer be combined
            /// with it, so the ordering of the combined rows can never be
            /// interleaved with a set operation.
            pub(super) fn tail_closed(
                self,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                SelectQuery {
                    ctes: self.ctes,
                    from: self.from,
                    projections: self.projections,
                    joins: self.joins,
                    derived: self.derived,
                    filter: self.filter,
                    group_by: self.group_by,
                    having: self.having,
                    order_by: self.order_by,
                    limit: self.limit,
                    offset: self.offset,
                    set_operations: self.set_operations,
                    distinct: self.distinct,
                    distinct_on: self.distinct_on,
                    compound_order_by: self.compound_order_by,
                    compound_limit: self.compound_limit,
                    compound_offset: self.compound_offset,
                    lock: self.lock,
                    _marker: PhantomData,
                }
            }
        }
    };
}

impl_tail_closed!(SetOpApplied);
impl_tail_closed!(TailClosed);

#[cfg(test)]
mod tests;
