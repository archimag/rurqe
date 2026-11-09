//! Ordering and pagination of a grouped query.
//!
//! A grouped statement produces one row per group, so the values it sorts by are
//! either grouped columns — taken by the `order_by` family — or aggregates
//! computed over each group, which the `order_by_expr` family takes. The
//! pagination methods bound the number of groups the statement returns.

use crate::dialect::{Dialect, SupportsNullsOrdering};
use crate::expr::{
    AggregateMarker, DialectInScope, Expr, NonAggregate, NullsOrder, OrderCriterion, OrderDirection,
};
use crate::schema::{Column, ExprInScope, TableInScope};

use super::super::scope::ScopePositions;
use super::super::selection::AggregateInDialect;
use super::{ColumnInGroupBy, GroupedQuery};

impl<D: Dialect, Tables, Output, GroupByCols, Projected>
    GroupedQuery<D, Tables, Output, GroupByCols, Projected>
{
    /// Appends an ordering criterion for the grouped column `col` with `direction`.
    ///
    /// A grouped query produces one row per group, so the values it sorts by are
    /// either grouped columns — which this method takes — or aggregates computed
    /// over each group, which [`order_by_expr`](GroupedQuery::order_by_expr) takes.
    /// Ordering by a column the query does not group by is therefore rejected at
    /// the call site.
    pub fn order_by<C: Column, P, PGroup>(mut self, col: C, direction: OrderDirection) -> Self
    where
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.query = self.query.order_by(col, direction);
        self
    }

    /// Appends an ascending ordering criterion for the grouped column `col`.
    pub fn order_by_asc<C: Column, P, PGroup>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.order_by(col, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for the grouped column `col`.
    pub fn order_by_desc<C: Column, P, PGroup>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.order_by(col, OrderDirection::Desc)
    }

    /// Appends an ordering criterion fixing where the NULL values of `col` sort.
    pub fn order_by_nulls<C: Column, P, PGroup>(
        mut self,
        col: C,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.query = self.query.order_by_nulls(col, direction, nulls);
        self
    }

    /// Appends an ascending ordering criterion putting NULL values first.
    pub fn order_by_asc_nulls_first<C: Column, P, PGroup>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.order_by_nulls(col, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion putting NULL values last.
    pub fn order_by_asc_nulls_last<C: Column, P, PGroup>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.order_by_nulls(col, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion putting NULL values first.
    pub fn order_by_desc_nulls_first<C: Column, P, PGroup>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.order_by_nulls(col, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion putting NULL values last.
    pub fn order_by_desc_nulls_last<C: Column, P, PGroup>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
    {
        self.order_by_nulls(col, OrderDirection::Desc, NullsOrder::Last)
    }

    /// Appends an ordering criterion for the aggregate `expression` with `direction`.
    ///
    /// The rows of a group are already collapsed when the statement sorts them, so
    /// the values it orders by are either grouped columns — ordered with
    /// [`order_by`](GroupedQuery::order_by) — or aggregates computed over each
    /// group, which is what this method takes:
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
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
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .group_by(OrderUserId)
    ///         .select((OrderUserId, Expr::count_all()))
    ///         .order_by_expr_desc(Expr::count_all())
    ///         .build()
    ///         .expect("valid grouped query");
    ///
    ///     assert_eq!(
    ///         built.sql,
    ///         "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
    ///          ORDER BY COUNT(*) DESC"
    ///     );
    /// }
    /// ```
    pub fn order_by_expr<T, Scope, Marker, ExprD, P>(
        mut self,
        expression: Expr<T, Scope, Marker, ExprD>,
        direction: OrderDirection,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        let criterion = OrderCriterion::new(expression.into_kind(), direction);
        self.query.push_ordering(criterion);
        self
    }

    /// Appends an ordering criterion for `expression` fixing where NULL values sort.
    pub fn order_by_expr_nulls<T, Scope, Marker, ExprD, P>(
        mut self,
        expression: Expr<T, Scope, Marker, ExprD>,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        let criterion = OrderCriterion::new(expression.into_kind(), direction).nulls(nulls);
        self.query.push_ordering(criterion);
        self
    }

    /// Appends an ascending ordering criterion for the aggregate `expression`.
    pub fn order_by_expr_asc<T, Scope, Marker, ExprD, P>(
        self,
        expression: Expr<T, Scope, Marker, ExprD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr(expression, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for the aggregate `expression`.
    pub fn order_by_expr_desc<T, Scope, Marker, ExprD, P>(
        self,
        expression: Expr<T, Scope, Marker, ExprD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr(expression, OrderDirection::Desc)
    }

    /// Appends an ascending ordering criterion for the aggregate `expression`
    /// putting NULL values first.
    pub fn order_by_expr_asc_nulls_first<T, Scope, Marker, ExprD, P>(
        self,
        expression: Expr<T, Scope, Marker, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion for the aggregate `expression`
    /// putting NULL values last.
    pub fn order_by_expr_asc_nulls_last<T, Scope, Marker, ExprD, P>(
        self,
        expression: Expr<T, Scope, Marker, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion for the aggregate `expression`
    /// putting NULL values first.
    pub fn order_by_expr_desc_nulls_first<T, Scope, Marker, ExprD, P>(
        self,
        expression: Expr<T, Scope, Marker, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion for the aggregate `expression`
    /// putting NULL values last.
    pub fn order_by_expr_desc_nulls_last<T, Scope, Marker, ExprD, P>(
        self,
        expression: Expr<T, Scope, Marker, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        ExprD: DialectInScope<D>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Desc, NullsOrder::Last)
    }
    /// Appends an ordering criterion for the grouped expression `expression` with `direction`.
    ///
    /// A statement grouped by a computed expression produces one value of that
    /// expression per group, so it orders the groups by the same expression the
    /// `GROUP BY` clause names: the criterion has to be structurally equal to a
    /// grouped expression, exactly as a projection has to be. The comparison runs
    /// while the statement is built, because it compares the assembled expressions —
    /// a variation is refused as
    /// [`UngroupedExpression`](crate::query::BuildError::UngroupedExpression), and a
    /// column the grouping dropped as
    /// [`UngroupedColumn`](crate::query::BuildError::UngroupedColumn).
    pub fn order_by_grouped_expr<T, Scope, ExprD, P>(
        mut self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
        direction: OrderDirection,
    ) -> Self
    where
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        let criterion = OrderCriterion::new(expression.into_kind(), direction);
        self.query.push_ordering(criterion);
        self
    }

    /// Appends an ordering criterion for the grouped expression `expression` fixing
    /// where NULL values sort.
    pub fn order_by_grouped_expr_nulls<T, Scope, ExprD, P>(
        mut self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        let criterion = OrderCriterion::new(expression.into_kind(), direction).nulls(nulls);
        self.query.push_ordering(criterion);
        self
    }

    /// Appends an ascending ordering criterion for the grouped expression `expression`.
    pub fn order_by_grouped_expr_asc<T, Scope, ExprD, P>(
        self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
    ) -> Self
    where
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        self.order_by_grouped_expr(expression, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for the grouped expression `expression`.
    pub fn order_by_grouped_expr_desc<T, Scope, ExprD, P>(
        self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
    ) -> Self
    where
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        self.order_by_grouped_expr(expression, OrderDirection::Desc)
    }

    /// Appends an ascending ordering criterion for the grouped expression `expression`
    /// putting NULL values first.
    pub fn order_by_grouped_expr_asc_nulls_first<T, Scope, ExprD, P>(
        self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        self.order_by_grouped_expr_nulls(expression, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion for the grouped expression `expression`
    /// putting NULL values last.
    pub fn order_by_grouped_expr_asc_nulls_last<T, Scope, ExprD, P>(
        self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        self.order_by_grouped_expr_nulls(expression, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion for the grouped expression `expression`
    /// putting NULL values first.
    pub fn order_by_grouped_expr_desc_nulls_first<T, Scope, ExprD, P>(
        self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        self.order_by_grouped_expr_nulls(expression, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion for the grouped expression `expression`
    /// putting NULL values last.
    pub fn order_by_grouped_expr_desc_nulls_last<T, Scope, ExprD, P>(
        self,
        expression: Expr<T, Scope, NonAggregate, ExprD>,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        T: 'static + Send + Sync,
        Scope: ExprInScope<Tables, P> + ScopePositions<P> + 'static + Send + Sync,
        ExprD: DialectInScope<D>,
    {
        self.order_by_grouped_expr_nulls(expression, OrderDirection::Desc, NullsOrder::Last)
    }
    /// Limits the number of returned groups to `limit`.
    pub fn limit(mut self, limit: u32) -> Self {
        self.query = self.query.limit(limit);
        self
    }

    /// Skips the first `offset` groups of the result.
    pub fn offset(mut self, offset: u32) -> Self {
        self.query = self.query.offset(offset);
        self
    }
}
