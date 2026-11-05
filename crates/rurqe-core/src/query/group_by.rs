//! GROUP BY typestate, grouped-column sets, and the grouped SELECT builder.

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{AggregateMarker, DialectInScope, Expr, NonAggregate, SelectAst};
use crate::schema::{At, Column, ExprInScope, Table};

use super::cte::{Cte, DerivedTable};
use super::scope::{ScopePositions, TableScope};
use super::select::{BuiltQuery, HasProjection, NoProjection, ProjectionPresent, SelectQuery};
use super::selection::{AggregateInDialect, SelectTarget, Selection, SelectionFor};

// The grouped query keeps one responsibility per file: column sets in `sets`, `HAVING`
// in `having`, the ordering and pagination of the grouped rows in `ordering`, and the
// window projection rules in `window_targets`.
mod having;
mod ordering;
mod sets;
mod window_targets;

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
///
/// A per-row predicate over a grouped expression is accepted by the call site as
/// well, because the call site cannot tell it from a predicate over a bare column.
/// The statement build compares the assembled predicate with the grouped
/// expressions and refuses a column the grouping does not cover as
/// [`BuildError::UngroupedHavingColumn`](crate::query::BuildError::UngroupedHavingColumn).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid HAVING predicate",
    label = "the predicate reads a column that is neither grouped nor aggregated",
    note = "filter the rows with `where_` before grouping, compare a grouped column with `having_col`, or wrap the column into an aggregate such as `count`, `sum`, `avg`, `min`, or `max`"
)]
pub trait HavingPredicate<GroupByCols, Tables, P = At<0>, D = ()> {}

impl<GroupByCols, Tables, Scope, Marker, ExprD, P, D> HavingPredicate<GroupByCols, Tables, P, D>
    for Expr<bool, Scope, Marker, ExprD>
where
    Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    Marker: AggregateMarker + AggregateInDialect<D>,
    ExprD: DialectInScope<D>,
{
}

impl<GroupByCols, Tables, Scope, ExprD, P, D> HavingPredicate<GroupByCols, Tables, P, D>
    for Expr<bool, Scope, NonAggregate, ExprD>
where
    Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    ExprD: DialectInScope<D>,
{
}

/// Checks that a projection item may appear in a `GROUP BY` query.
///
/// The trait is a membership rule, not a type derivation: it states that a
/// column is part of the `GROUP BY` clause, and that an aggregate, a constant
/// expression, or a window expression is accepted beside the grouped columns.
/// [`GroupedSelection`](crate::query::GroupedSelection) applies the rule to the
/// tuple a selection is made of, while the row type of the selection is derived
/// by [`SelectionFor`](crate::query::SelectionFor) alone, exactly as for an
/// ungrouped statement.
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
/// rurqe_core::table!(
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// );
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
pub trait GroupedSelectTarget<GroupByCols, P = At<0>, D = ()>: SelectTarget {}

impl<GroupByCols, C, P, D> GroupedSelectTarget<GroupByCols, P, D> for C
where
    C: Column,
    GroupByCols: ColumnInGroupBy<C, P>,
{
}

/// Accepts an aggregate as a projection of a `GROUP BY` query.
///
/// An aggregate collapses the rows of a group into a single value, so it may
/// reference a column the query does not group by.
impl<GroupByCols, T, Scope, Marker, ExprD, D> GroupedSelectTarget<GroupByCols, At<0>, D>
    for Expr<T, Scope, Marker, ExprD>
where
    T: 'static + Send + Sync,
    Scope: 'static + Send + Sync,
    Marker: AggregateMarker + AggregateInDialect<D>,
    ExprD: DialectInScope<D>,
{
}

/// Accepts a per-row expression as a projection of a `GROUP BY` query.
///
/// A literal, a raw SQL fragment such as `sql!(<i64> "COUNT(*)")`, or a
/// comparison between them reads no table and is constant within a group. An
/// expression that reads a column keeps the scope of that column, and the call
/// site cannot tell a bare column wrapped into an expression from an expression
/// computed over several columns. The rule that keeps the grouping sound is
/// therefore decided while the statement is built, on the assembled expressions:
/// a projected expression has to equal one of the grouped expressions, or read
/// grouped columns only. A column the clause dropped is refused as
/// [`BuildError::UngroupedColumn`](crate::query::BuildError::UngroupedColumn), and
/// an expression computed over it as
/// [`BuildError::UngroupedExpression`](crate::query::BuildError::UngroupedExpression)
/// when the statement groups by a computed expression, before any SQL is rendered.
///
/// A `CASE` expression propagates the rule through its branches: a branch reading
/// a bare column keeps the whole expression per-row, so the statement build
/// refuses it unless the column is grouped:
///
/// ```
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::expr::Expr;
/// use rurqe_core::query::{BuildError, SelectQuery};
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
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     // The CASE branch reads `total`, which is not grouped.
///     let built = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((
///             OrderUserId,
///             Expr::<f64>::case()
///                 .when(Expr::column::<OrderTotal>().gt(0.0), 1.0)
///                 .else_(0.0)
///                 .end(),
///         ))
///         .build();
///     assert!(matches!(built, Err(BuildError::UngroupedColumn { .. })));
/// }
/// ```
///
/// Dropping the scope with
/// [`Expr::without_scope`](crate::expr::Expr::without_scope) — the escape hatch
/// correlated subqueries are written with — replaces the tables with the
/// [`Unscoped`](crate::expr::Unscoped) marker instead of the empty scope `()`, and
/// qualifies the references it carries, so the statement build still sees the
/// column the expression reads:
///
/// ```
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::expr::Expr;
/// use rurqe_core::query::{BuildError, SelectQuery};
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
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     // The expression still reads `total`, which is not grouped.
///     let built = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((OrderUserId, Expr::column::<OrderTotal>().without_scope()))
///         .build();
///     assert!(matches!(built, Err(BuildError::UngroupedColumn { .. })));
/// }
/// ```
impl<GroupByCols, T, Scope, ExprD, D> GroupedSelectTarget<GroupByCols, At<0>, D>
    for Expr<T, Scope, NonAggregate, ExprD>
where
    T: 'static + Send + Sync,
    Scope: 'static + Send + Sync,
    ExprD: DialectInScope<D>,
{
}
/// Checks that every projection item of a selection may appear in a grouped query.
///
/// The trait extends the membership rule of [`GroupedSelectTarget`] to the tuple
/// a selection is made of and derives no row type of its own: the row of a
/// grouped selection is the one [`SelectionFor`](crate::query::SelectionFor)
/// derives in the scope of the statement, so it matches the row an ungrouped
/// statement with the same selection produces, outer-join nullability included.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be selected in a GROUP BY query",
    label = "one of the projected columns is neither grouped nor an aggregate",
    note = "add the column to `group_by` or wrap it into an aggregate such as `count`, `sum`, `avg`, `min`, or `max`"
)]
pub trait GroupedSelection<GroupByCols, Positions, D = ()>: Selection {}

macro_rules! impl_grouped_selection {
    ($($target:ident $position:ident),+) => {
        impl<GroupByCols, D, $($target, $position),+>
            GroupedSelection<GroupByCols, ($($position,)+), D> for ($($target,)+)
        where
            $($target: GroupedSelectTarget<GroupByCols, $position, D>,)+
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
/// it projects its rows through [`GroupedQuery::select`]. The grouped typestate
/// is final: a grouped statement is embedded into a larger statement through
/// [`GroupedQuery::as_cte`] or [`GroupedQuery::as_table`], and it never degrades
/// back into a plain `SelectQuery` that would still render a `GROUP BY` clause
/// the builder chain could no longer check.
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
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .group_by(OrderUserId)
    ///         .select((OrderUserId, Expr::sum::<OrderTotal>()))
    ///         .build()
    ///         .expect("valid grouped query");
    ///
    ///     assert_eq!(
    ///         built.sql,
    ///         "SELECT \"user_id\", SUM(\"total\") FROM \"orders\" GROUP BY \"user_id\""
    ///     );
    /// }
    /// ```
    ///
    /// The `Expr` escape hatch does not bypass the rule, because an expression
    /// referencing a column keeps the table of that column. The call site accepts
    /// the expression — it cannot tell a bare column from one computed over several
    /// — and the statement build refuses it:
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::{BuildError, SelectQuery};
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
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // `OrderTotal` is neither grouped nor an aggregate.
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .group_by(OrderUserId)
    ///         .select((OrderUserId, Expr::column::<OrderTotal>()))
    ///         .build();
    ///     assert!(matches!(built, Err(BuildError::UngroupedColumn { .. })));
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
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// );
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
    ///
    /// The selection is consumed where it is written: its projection nodes are
    /// drained into the statement at the call site and nothing of the selection is
    /// stored, so a selection borrowing its state — one holding a `&str`, for
    /// instance — is accepted exactly like an owned one. Only the two semantic
    /// contracts bound it, matching the bounds `SelectQuery::select` imposes on the
    /// same trait: it has to be projectable from the scope of the statement
    /// (`SelectionFor`), and every item it records has to name a grouped column, an
    /// aggregate, or a constant (`GroupedSelection`).
    ///
    /// The row type of the statement is the one the selection produces in the
    /// query scope, exactly like an ungrouped statement: a column read through a
    /// `LEFT JOIN` stays `Option<T>` in a grouped row as well, and the grouping
    /// rule adds no second derivation of it.
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::{GroupedQuery, SelectQuery};
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
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // `OrderTotal` is read from the nullable side of the join, so the
    ///     // grouped row keeps it as `Option<f64>`.
    ///     let _: GroupedQuery<MockDialect, _, (i64, Option<f64>), _> =
    ///         SelectQuery::<MockDialect>::new()
    ///             .from::<User>()
    ///             .left_join(Order, Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
    ///             .group_by((UserId, OrderTotal))
    ///             .select((UserId, OrderTotal));
    /// }
    /// ```
    pub fn select<S, Positions, GroupPositions>(
        self,
        selection: S,
    ) -> GroupedQuery<
        D,
        Tables,
        <S as SelectionFor<Tables, Positions, D>>::Row,
        GroupByCols,
        HasProjection,
    >
    where
        S: SelectionFor<Tables, Positions, D> + GroupedSelection<GroupByCols, GroupPositions, D>,
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
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// );
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
        G: GroupBy + GroupByInScope<Tables, Positions, D> + GroupByColumns<GroupByCols>,
    {
        let mut query = self.query;
        columns.push_column_names(&mut query.group_by);
        GroupedQuery {
            query,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, Tables: TableScope, Output, GroupByCols, P: ProjectionPresent>
    GroupedQuery<D, Tables, Output, GroupByCols, P>
{
    /// Validates pre-flight SQL semantics and renders the grouped statement into SQL text and its ordered parameter list.
    ///
    /// Like [`SelectQuery::build`], the method is reachable only once the query
    /// projects its rows, so a grouped query without a projection never renders.
    pub fn build(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.query.build()
    }
}

impl<D: Dialect, Tables: TableScope, Output, GroupByCols>
    GroupedQuery<D, Tables, Output, GroupByCols, HasProjection>
{
    /// Describes this grouped query as a statement AST, the way the nested
    /// projections embed it, without leaving the grouped typestate.
    pub(crate) fn to_ast(&self) -> SelectAst {
        self.query.to_ast()
    }

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
