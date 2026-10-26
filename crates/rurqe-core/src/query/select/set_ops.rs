//! Set-operation builders of the `SELECT` query.

use crate::dialect::Dialect;
use crate::expr::{SetOpAst, SetOpKind};
use crate::query::scope::{Compound, TableScope};

use super::{HasProjection, SelectQuery, SetOpApplied, SetOperand};

impl<D: Dialect, Tables, Output, Lock: SetOperand>
    SelectQuery<D, Tables, Output, HasProjection, Lock>
{
    /// Combines this query with `other` using `UNION`, removing duplicate rows.
    ///
    /// The combined statement produces the output columns of its first projection
    /// list, so the query moves into the [`Compound`] scope and carries
    /// [`SetOpApplied`]: its rows no longer belong to a table, its projection is
    /// recorded for good — [`select`](SelectQuery::select) stays unreachable — and
    /// the ordering of the combined rows goes through `order_by_output` or
    /// `order_by_index`. A combined statement can neither keep the row-level lock
    /// of an operand nor accept a new one, so the first criterion written after it
    /// closes the statement for further operands.
    pub fn union<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, SetOpApplied> {
        self.with_set_operation(SetOpKind::Union, other)
    }

    /// Combines this query with `other` using `UNION ALL`, keeping duplicates.
    ///
    /// The result carries the scope, the lock state, and the ordering tail of a
    /// [`union`](Self::union).
    pub fn union_all<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, SetOpApplied> {
        self.with_set_operation(SetOpKind::UnionAll, other)
    }

    /// Keeps only the rows produced by both this query and `other` (`INTERSECT`).
    pub fn intersect<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, SetOpApplied> {
        self.with_set_operation(SetOpKind::Intersect, other)
    }

    /// Keeps the rows of this query that are absent from `other` (`EXCEPT`).
    pub fn except<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, SetOpApplied> {
        self.with_set_operation(SetOpKind::Except, other)
    }

    fn with_set_operation<Tables2: TableScope>(
        mut self,
        kind: SetOpKind,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, SetOpApplied> {
        // The combined query is consumed, so its clauses move into the operand
        // instead of being copied.
        self.set_operations.push(SetOpAst {
            kind,
            query: other.into_ast(),
        });
        self.retype()
    }
}
