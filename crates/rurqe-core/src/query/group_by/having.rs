//! The `HAVING` clause of a grouped query.
//!
//! `HAVING` filters the groups an aggregate produced, so the clause reads an
//! aggregate or an expression that is constant within a group. The contract
//! stating that rule lives in the parent module as `HavingPredicate`.

use crate::dialect::Dialect;
use crate::expr::{DialectInScope, Expr, NonAggregate};
use crate::schema::{Column, ExprInScope, TableInScope};

use super::super::scope::ScopePositions;
use super::super::selection::AggregateInDialect;
use super::{ColumnInGroupBy, GroupedQuery, HavingPredicate};

impl<D: Dialect, Tables, Output, GroupByCols, Projected>
    GroupedQuery<D, Tables, Output, GroupByCols, Projected>
{
    /// Restricts grouped rows to those satisfying `predicate`.
    ///
    /// A `HAVING` clause filters the groups an aggregate produced, so the
    /// predicate is written over an aggregate — `count_all().gt(5i64)`, for
    /// instance — and keeps the aggregation marker of that comparison. A predicate
    /// that is constant within a group, such as a `sql!` fragment comparing two
    /// aggregates, is accepted as well, while a predicate reading a bare column is
    /// rejected: the grouping dropped the per-row value that column would name.
    /// Compare grouped columns with [`having_col`](GroupedQuery::having_col)
    /// instead, and filter rows before grouping with `where_`.
    ///
    /// The clause exists only on a grouped query, so a statement that never
    /// recorded its grouping cannot reach it:
    ///
    /// compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: the query never grouped its rows, so it has no
    ///     // `having` clause to record.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .select((OrderUserId,))
    ///         .having(Expr::count_all().gt(1i64));
    /// }
    /// 
    pub fn having<Scope, Aggr, ExprD, P>(
        mut self,
        predicate: Expr<bool, Scope, Aggr, ExprD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Expr<bool, Scope, Aggr, ExprD>: HavingPredicate<GroupByCols, Tables, P, D>,
        Aggr: AggregateInDialect<D>,
    {
        self.query = self.query.having(predicate);
        self
    }

    /// Restricts grouped rows by `predicate` written over the grouped column `col`.
    ///
    /// A grouped column keeps one value per group, so it is compared directly
    /// instead of through an aggregate:
    /// `having_col(OrderUserId, OrderUserId.gt(10i64))` keeps the groups of the
    /// users above ten. The column must be part of the `GROUP BY` clause, which is
    /// exactly what a bare column reference inside
    /// [`having`](GroupedQuery::having) lacks.
    pub fn having_col<C, ExprD, PGroup, PTable>(
        mut self,
        _col: C,
        predicate: Expr<bool, C::Table, NonAggregate, ExprD>,
    ) -> Self
    where
        ExprD: DialectInScope<D>,
        C: Column,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
        Tables: TableInScope<C::Table, PTable>,
    {
        self.query = self.query.having(predicate);
        self
    }
}
