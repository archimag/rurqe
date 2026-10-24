//! GROUP BY typestate, grouped-column sets, and the grouped SELECT builder.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsNullsOrdering};
use crate::expr::{AggregateMarker, Expr, NonAggregate, NullsOrder, OrderCriterion};
use crate::schema::{At, Column, ExprInScope, Table, TableInScope};

use super::cte::{Cte, DerivedTable};
use super::scope::{ScopePositions, TableScope};
use super::select::{
    BuiltQuery, HasProjection, LockUnavailable, NoProjection, OrderDirection, ProjectionPresent,
    SelectQuery,
};
use super::selection::{AggregateInDialect, SelectTarget, Selection, SelectionFor};
use super::window::WindowExpr;

// The type-level column sets and the membership checks live in a sibling module, so
// the grouped query builder keeps one responsibility per file.
mod sets;

pub use self::sets::{
    ColumnInGroupBy, EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupSet,
    GroupSetColumns, WindowColsInGroupBy,
};

/// Statically asserts that a `HAVING` predicate filters the groups of a query.
///
/// The rows of a grouped query are collapsed into groups before `HAVING` runs, so
/// the clause reads either an aggregate computed over a group —
/// `count_all().gt(5i64)`, for instance — or an expression that is constant within
/// a group, such as a `sql!` fragment comparing two aggregates. A predicate
/// reading a bare column is rejected, because the grouping dropped the per-row
/// value that column would name: such rows are filtered with `where_` before
/// grouping, and grouped columns are compared through
/// [`GroupedQuery::having_col`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid HAVING predicate",
    label = "the predicate reads a column that is neither grouped nor aggregated",
    note = "filter the rows with `where_` before grouping, compare a grouped column with `having_col`, or wrap the column into an aggregate such as `count`, `sum`, `avg`, `min`, or `max`"
)]
pub trait HavingPredicate<GroupByCols, Tables, P = At<0>> {}

impl<GroupByCols, Tables, Scope, Marker, P> HavingPredicate<GroupByCols, Tables, P>
    for Expr<bool, Scope, Marker>
where
    Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    Marker: AggregateMarker,
{
}

impl<GroupByCols, Tables, P> HavingPredicate<GroupByCols, Tables, P>
    for Expr<bool, (), NonAggregate>
{
}

/// Derives the row item produced by a projection in a `GROUP BY` query.
///
/// A plain column reaches the row only once it is part of the `GROUP BY` clause,
/// while an aggregate collapses the rows of a group and stays projectable even
/// when it reads a column the query does not group by. Projecting a column that
/// is neither grouped nor aggregated is therefore rejected at the call site:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
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
/// rurqe_core::table! {
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: `OrderTotal` is neither grouped nor an aggregate.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((OrderUserId, OrderTotal));
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "column `{Self}` cannot be selected in a GROUP BY query because it is neither grouped nor an aggregate",
    label = "the projected column is neither part of the GROUP BY clause nor an aggregate",
    note = "add the column to `group_by` or wrap it into an aggregate such as `count`, `sum`, `avg`, `min`, or `max`"
)]
pub trait GroupedSelectTarget<GroupByCols, P = At<0>>: SelectTarget {
    /// The Rust type produced for this projection in a grouped query.
    type Output;
}

impl<GroupByCols, C, P> GroupedSelectTarget<GroupByCols, P> for C
where
    C: Column,
    GroupByCols: ColumnInGroupBy<C, P>,
{
    type Output = C::RustType;
}

/// Derives the row item produced by an aggregate in a `GROUP BY` query.
///
/// An aggregate collapses the rows of a group into a single value, so it may
/// reference a column the query does not group by.
impl<GroupByCols, T, Scope, Marker> GroupedSelectTarget<GroupByCols, At<0>>
    for Expr<T, Scope, Marker>
where
    T: 'static + Send + Sync,
    Scope: 'static + Send + Sync,
    Marker: AggregateMarker,
{
    type Output = T;
}

/// Derives the row item produced by a constant expression in a `GROUP BY` query.
///
/// An expression referencing no table — a literal, a raw SQL fragment such as
/// `sql!(<i64> "COUNT(*)")`, or a comparison between them — is constant within a
/// group, so a grouped query accepts it even though it is not an aggregate.
/// An expression that reads a column keeps the scope of that column instead:
/// [`coalesce`](crate::expr::coalesce) merges the scope of its column with the
/// scope of its fallback, and [`Expr::case`](crate::expr::Expr::case) merges the
/// scopes of the conditions and results of its branches. Both therefore stay
/// rejected unless the columns they read are grouped.
///
/// Dropping the scope with
/// [`Expr::without_scope`](crate::expr::Expr::without_scope) — the escape hatch
/// correlated subqueries are written with — replaces the tables with the
/// [`Unscoped`](crate::expr::Unscoped) marker instead of the empty scope `()`, so
/// a dropped column reference never turns the expression into a constant:
///
/// ```compile_fail
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
/// rurqe_core::table! {
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: the expression still reads `total`, which is not grouped.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((OrderUserId, Expr::column::<OrderTotal>().without_scope()));
/// }
/// ```
impl<GroupByCols, T: 'static + Send + Sync> GroupedSelectTarget<GroupByCols, At<0>>
    for Expr<T, (), NonAggregate>
{
    type Output = T;
}

/// Derives the row item produced by a window expression in a `GROUP BY` query.
///
/// A window function is evaluated over the rows of a group once the rows are
/// grouped, so the columns its specification partitions by and orders by have to
/// survive the grouping as well: the database evaluates `PARTITION BY` and
/// `ORDER BY` after `GROUP BY` and cannot read a column the grouping dropped. The
/// column set `Cols` recorded by the window expression is therefore checked
/// against the grouped columns through [`WindowColsInGroupBy`], so a window
/// reading an ungrouped column is rejected at the call site.
impl<
        GroupByCols,
        T: 'static + Send + Sync,
        Scope: 'static + Send + Sync,
        Cols: 'static + Send + Sync,
        D,
        Positions,
    > GroupedSelectTarget<GroupByCols, Positions> for WindowExpr<T, Scope, Cols, D>
where
    Cols: WindowColsInGroupBy<GroupByCols, Positions>,
{
    type Output = T;
}

/// Derives the row type of a selection projected from a grouped query.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be selected in a GROUP BY query",
    label = "one of the projected columns is neither grouped nor an aggregate",
    note = "add the column to `group_by` or wrap it into an aggregate such as `count`, `sum`, `avg`, `min`, or `max`"
)]
pub trait GroupedSelection<GroupByCols, Positions>: Selection {}

macro_rules! impl_grouped_selection {
    ($($target:ident $position:ident),+) => {
        impl<GroupByCols, $($target, $position),+> GroupedSelection<GroupByCols, ($($position,)+)>
            for ($($target,)+)
        where
            $($target: GroupedSelectTarget<GroupByCols, $position>,)+
        {
        }
    };
}

impl_grouped_selection!(T1 P1);
impl_grouped_selection!(T1 P1, T2 P2);
impl_grouped_selection!(T1 P1, T2 P2, T3 P3);
impl_grouped_selection!(T1 P1, T2 P2, T3 P3, T4 P4);
impl_grouped_selection!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5);
impl_grouped_selection!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6);
impl_grouped_selection!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6, T7 P7);
impl_grouped_selection!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6, T7 P7, T8 P8);

/// A `SELECT` query whose rows are grouped by the columns recorded in `GroupByCols`.
///
/// `Projected` mirrors [`SelectQuery`]: a grouped query is renderable only once
/// it projects its rows through [`GroupedQuery::select`].
pub struct GroupedQuery<
    D: Dialect,
    Tables = (),
    Output = (),
    GroupByCols = EmptyGroupSet,
    Projected = HasProjection,
> {
    pub(crate) query: SelectQuery<D, Tables, Output, Projected>,
    pub(crate) _marker: PhantomData<fn() -> GroupByCols>,
}

impl<D: Dialect, Tables: TableScope, GroupByCols>
    GroupedQuery<D, Tables, (), GroupByCols, NoProjection>
{
    /// Selects `selection`, requiring every projected column to be grouped or aggregated.
    ///
    /// A column reference must therefore be part of the `GROUP BY` clause, while
    /// an aggregate such as `count`, `sum`, `avg`, `min`, or `max` may reference
    /// any column of the query scope. Every projected item stays subject to the
    /// query scope as well, so a window expression partitioned by a table the
    /// query does not select from is rejected like any other projection.
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .group_by(OrderUserId)
    ///         .select((OrderUserId, Expr::sum::<OrderTotal>()))
    ///         .build();
    ///
    ///     assert_eq!(
    ///         built.sql,
    ///         "SELECT \"user_id\", SUM(\"total\") FROM \"orders\" GROUP BY \"user_id\""
    ///     );
    /// }
    /// ```
    ///
    /// The `Expr` escape hatch does not bypass the rule, because an expression
    /// referencing a column keeps the scope of its table:
    ///
    /// ```compile_fail
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: `OrderTotal` is neither grouped nor an aggregate.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .group_by(OrderUserId)
    ///         .select((OrderUserId, Expr::column::<OrderTotal>()));
    /// }
    /// ```
    ///
    /// Window expressions are checked against the scope of the grouped query, so
    /// a specification referencing an unjoined table is rejected:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::query::{row_number, SelectQuery, Window};
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
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: `OrderUserId` belongs to `Order`, which the query
    ///     // does not select from.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .group_by(UserId)
    ///         .select((UserId, row_number().over(Window::partition_by(OrderUserId))));
    /// }
    /// ```
    pub fn select<S, Positions, GroupPositions>(
        self,
        selection: S,
    ) -> GroupedQuery<
        D,
        Tables,
        <S as SelectionFor<Tables, Positions>>::Row,
        GroupByCols,
        HasProjection,
    >
    where
        S: SelectionFor<Tables, Positions>
            + GroupedSelection<GroupByCols, GroupPositions>
            + Send
            + Sync
            + 'static,
    {
        GroupedQuery {
            query: self.query.select::<S, Positions>(selection),
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables, GroupByCols> GroupedQuery<D, Tables, (), GroupByCols, NoProjection> {
    /// Groups the rows further by `columns`, appending them to the grouping.
    ///
    /// Further grouping is only permitted before projections are fixed.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .group_by(OrderUserId)
    ///         .select((OrderUserId,))
    ///         .group_by(OrderUserId);
    /// }
    /// ```
    pub fn group_by<G, Positions>(
        self,
        columns: G,
    ) -> GroupedQuery<D, Tables, (), <G as GroupByColumns<GroupByCols>>::Columns, NoProjection>
    where
        G: GroupBy + GroupByInScope<Tables, Positions> + GroupByColumns<GroupByCols>,
    {
        let mut query = self.query;
        columns.push_column_names(&mut query.group_by);
        GroupedQuery {
            query,
            _marker: PhantomData,
        }
    }
}

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
    pub fn having<Scope, Aggr, P>(mut self, predicate: Expr<bool, Scope, Aggr>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        Expr<bool, Scope, Aggr>: HavingPredicate<GroupByCols, Tables, P>,
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
    pub fn having_col<C, PGroup, PTable>(mut self, _col: C, predicate: Expr<bool, C::Table>) -> Self
    where
        C: Column,
        GroupByCols: ColumnInGroupBy<C, PGroup>,
        Tables: TableInScope<C::Table, PTable>,
    {
        self.query = self.query.having(predicate);
        self
    }

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
    /// ```text
    /// let query = select_from(Order)
    ///     .group_by(OrderUserId)
    ///     .select((OrderUserId, Expr::count_all()))
    ///     .order_by_expr_desc(Expr::count_all());
    /// ```
    pub fn order_by_expr<T, Scope, Marker, P>(
        mut self,
        expression: Expr<T, Scope, Marker>,
        direction: OrderDirection,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        let criterion = OrderCriterion::new(expression.into_kind(), direction);
        self.query.push_ordering(criterion);
        self
    }

    /// Appends an ordering criterion for `expression` fixing where NULL values sort.
    pub fn order_by_expr_nulls<T, Scope, Marker, P>(
        mut self,
        expression: Expr<T, Scope, Marker>,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        let criterion = OrderCriterion::new(expression.into_kind(), direction).nulls(nulls);
        self.query.push_ordering(criterion);
        self
    }

    /// Appends an ascending ordering criterion for the aggregate `expression`.
    pub fn order_by_expr_asc<T, Scope, Marker, P>(self, expression: Expr<T, Scope, Marker>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr(expression, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for the aggregate `expression`.
    pub fn order_by_expr_desc<T, Scope, Marker, P>(self, expression: Expr<T, Scope, Marker>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        Marker: AggregateMarker + AggregateInDialect<D>,
    {
        self.order_by_expr(expression, OrderDirection::Desc)
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

    /// Returns the query of this state, dropping the grouping marker.
    ///
    /// A grouped statement produces one row per group, so the returned query
    /// carries `LockUnavailable`: PostgreSQL refuses a row-level lock on a
    /// statement whose rows are grouped, and the marker keeps
    /// [`for_update`](SelectQuery::for_update) and
    /// [`for_share`](SelectQuery::for_share) unreachable on it.
    pub fn into_select(self) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable> {
        self.query.retype()
    }
}

impl<D: Dialect, Tables: TableScope, Output, GroupByCols, P: ProjectionPresent>
    GroupedQuery<D, Tables, Output, GroupByCols, P>
{
    /// Renders the grouped statement into SQL text and its ordered parameter list.
    ///
    /// Like [`SelectQuery::build`], the method is reachable only once the query
    /// projects its rows, so a grouped query without a projection never renders.
    pub fn build(&self) -> BuiltQuery {
        self.query.build()
    }
}

impl<D: Dialect, Tables: TableScope, Output, GroupByCols>
    GroupedQuery<D, Tables, Output, GroupByCols, HasProjection>
{
    /// Converts this grouped query into a CTE for the table `T`.
    pub fn as_cte<T: Table>(self) -> Cte<T, Output, D> {
        Cte::new(self.query)
    }

    /// Exposes this grouped query as a derived table referenced through the marker `T`.
    ///
    /// An aggregated subquery is the usual source of a derived relation: the
    /// grouping stays inside the parenthesised statement, so the enclosing query
    /// reads one row per group instead of one row per aggregated source row.
    pub fn as_table<T: Table>(self) -> DerivedTable<T, Output, D> {
        DerivedTable::new(self.query)
    }
}

#[cfg(test)]
mod tests;
