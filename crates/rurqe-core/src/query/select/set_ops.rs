//! Set-operation builders of the `SELECT` query.

use crate::dialect::Dialect;
use crate::expr::{SetOpAst, SetOpKind};
use crate::query::scope::TableScope;

use super::{HasProjection, LockUnavailable, SelectQuery, SetOperand};

impl<D: Dialect, Tables, Output, Lock: SetOperand>
    SelectQuery<D, Tables, Output, HasProjection, Lock>
{
    /// Combines this query with `other` using `UNION`, removing duplicate rows.
    ///
    /// A combined statement can neither keep the row-level lock of an operand nor
    /// accept a new one, so the resulting query carries [`LockUnavailable`]. The
    /// ordering, the limit, and the offset written after the combination apply to
    /// the combined rows and therefore read the output column names of the
    /// projections rather than the relations of the left query.
    pub fn union<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Tables, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::Union, other)
    }

    /// Combines this query with `other` using `UNION ALL`, keeping duplicates.
    pub fn union_all<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Tables, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::UnionAll, other)
    }

    /// Keeps only the rows produced by both this query and `other` (`INTERSECT`).
    pub fn intersect<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Tables, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::Intersect, other)
    }

    /// Keeps the rows of this query that are absent from `other` (`EXCEPT`).
    pub fn except<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Tables, Output, HasProjection, LockUnavailable> {
        self.with_set_operation(SetOpKind::Except, other)
    }

    fn with_set_operation<Tables2: TableScope>(
        mut self,
        kind: SetOpKind,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> SelectQuery<D, Tables, Output, HasProjection, LockUnavailable> {
        self.set_operations.push(SetOpAst {
            kind,
            query: other.to_ast(),
        });
        self.retype()
    }
}
