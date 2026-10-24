//! Set-operation builders of the `SELECT` query.

use crate::dialect::Dialect;
use crate::expr::{SetOpAst, SetOpKind};
use crate::query::scope::{Compound, TableScope};

use super::{HasProjection, LockUnavailable, SelectQuery, SetOperand};

impl<D: Dialect, Tables, Output, Lock: SetOperand>
    SelectQuery<D, Tables, Output, HasProjection, Lock>
{
    /// Combines this query with `other` using `UNION`, removing duplicate rows.
    ///
    /// The combined statement produces the output columns of its first projection
    /// list, so the query moves into the [`Compound`] scope: its rows no longer
    /// belong to a table, and the ordering of the combined rows goes through
    /// `order_by_output` or `order_by_index`. A combined statement can neither
    /// keep the row-level lock of an operand nor accept a new one, so the query
    /// carries [`LockUnavailable`], and the first criterion written after it closes
    /// the statement for further operands.
    pub fn union<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::Union, other)
    }

    /// Combines this query with `other` using `UNION ALL`, keeping duplicates.
    ///
    /// The result carries the scope, the lock state, and the ordering tail of a
    /// [`union`](Self::union).
    pub fn union_all<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::UnionAll, other)
    }

    /// Keeps only the rows produced by both this query and `other` (`INTERSECT`).
    pub fn intersect<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::Intersect, other)
    }

    /// Keeps the rows of this query that are absent from `other` (`EXCEPT`).
    pub fn except<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::Except, other)
    }

    fn with_set_operation<Tables2: TableScope>(
        mut self,
        kind: SetOpKind,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Compound, Output, HasProjection, LockUnavailable> {
        self.set_operations.push(SetOpAst {
            kind,
            query: other.to_ast(),
        });
        self.retype()
    }
}
