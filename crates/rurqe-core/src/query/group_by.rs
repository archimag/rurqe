//! GROUP BY typestate, grouped-column sets, and the grouped SELECT builder.

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{AggregateMarker, DialectInScope, Expr, NonAggregate, SelectAst};
use crate::schema::{At, Column, ExprInScope, Table};

use super::cte::{Cte, DerivedTable};
use super::scope::{ScopePositions, TableScope};
use super::select::{BuiltQuery, HasProjection, NoProjection, ProjectionPresent, SelectQuery};
use super::selection::{AggregateInDialect, SelectTarget, Selection, SelectionFor};
use super::window::WindowExpr;

// The grouped query keeps one responsibility per file: column sets in `sets`, `HAVING`
// in `having`, and the ordering and pagination of the grouped rows in `ordering`.
mod having;
mod ordering;
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
pub trait HavingPredicate<GroupByCols, Tables, P = At<0>, D = ()> {}

impl<GroupByCols, Tables, Scope, Marker, ExprD, P, D> HavingPredicate<GroupByCols, Tables, P, D>
    for Expr<bool, Scope, Marker, ExprD>
where
    Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    Marker: AggregateMarker + AggregateInDialect<D>,
    ExprD: DialectInScope<D>,
{
}

impl<GroupByCols, Tables, ExprD, P, D> HavingPredicate<GroupByCols, Tables, P, D>
    for Expr<bool, (), NonAggregate, ExprD>
where
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

/// Accepts a constant expression as a projection of a `GROUP BY` query.
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
/// A `CASE` expression propagates the same rule through its branches: a branch
/// reading a bare column keeps the whole expression per-row, so a grouped query
/// rejects it unless the column is grouped:
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
/// rurqe_core::table!(
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     // Fails to compile: the CASE branch reads `total`, which is not grouped.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((
///             OrderUserId,
///             Expr::<f64>::case()
///                 .when(Expr::column::<OrderTotal>().gt(0.0), 1.0)
///                 .else_(0.0)
///                 .end(),
///         ));
/// }
/// ```
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
/// rurqe_core::table!(
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     // Fails to compile: the expression still reads `total`, which is not grouped.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((OrderUserId, Expr::column::<OrderTotal>().without_scope()));
/// }
/// ```
impl<GroupByCols, T, ExprD, D> GroupedSelectTarget<GroupByCols, At<0>, D>
    for Expr<T, (), NonAggregate, ExprD>
where
    T: 'static + Send + Sync,
    ExprD: DialectInScope<D>,
{
}

/// Accepts a window expression over an aggregating operand in a `GROUP BY` query.
///
/// A window function is evaluated over the rows of a group once the rows are
/// grouped, so the columns its specification partitions by and orders by have to
/// survive the grouping as well: the database evaluates `PARTITION BY` and
/// `ORDER BY` after `GROUP BY` and cannot read a column the grouping dropped. The
/// column set `Cols` recorded by the window expression is therefore checked
/// against the grouped columns through [`WindowColsInGroupBy`], so a window
/// reading an ungrouped column is rejected at the call site.
///
/// The expression the window evaluates over is an operand of the statement like
/// any other: the grouping collapses the rows of a group before the window runs,
/// so the operand keeps one value per group only when it aggregates those rows —
/// `SUM("total") OVER (...)` collapses its group exactly like the bare
/// `SUM("total")` does — or reads no table at all. The aggregation marker of the
/// operand is therefore checked through [`AggregateMarker`], which a per-row
/// operand never satisfies:
///
/// ```compile_fail
/// use rurqe_core::dialect::{Dialect, SupportsWindowFunctions};
/// use rurqe_core::expr::Expr;
/// use rurqe_core::query::{SelectQuery, Window};
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
/// impl SupportsWindowFunctions for MockDialect {}
///
/// rurqe_core::table!(
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     // Fails to compile: the window reads `total` of every row of the group,
///     // and the query never aggregates it.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((
///             OrderUserId,
///             Expr::column::<OrderTotal>().over(Window::partition_by(OrderUserId)),
///         ));
/// }
/// ```
///
/// An aggregate operand and a constant operand both keep one value per group, so
/// both stay projectable, and so does a grouped column read by `LAG` or `LEAD`,
/// whose column is checked through the window column set instead. An aggregate
/// operand is an operand of the statement like any other, so the columns it reads
/// have to survive the grouping as well: the example below aggregates a grouped
/// column, while `SUM("total") OVER (...)` over an ungrouped `"total"` names a
/// per-row value the grouped query never reports and is rejected while the
/// statement is built.
///
/// The grouping coverage extends to the columns a window aggregate reads inside a
/// composite operand. The walk collects them through every node the operand is
/// built from — a scalar function call, an arithmetic combination, a `CASE`
/// branch, a `CAST` — so `COALESCE(SUM("total"), $1) OVER (...)` keeps `"total"`
/// subject to the grouping and the statement build reports it as
/// [`BuildError::UngroupedColumn`](crate::query::BuildError::UngroupedColumn) when
/// the clause does not group it. The walk stops at an aggregate nested inside the
/// operand, which keeps `SUM(SUM("total")) OVER (...)` valid: the inner call
/// collapses the groups before the window reads them.
///
/// The aggregation markers describe the operand only as far as its grouping
/// behaviour, so the operand grammar is checked beside them while the statement is
/// built: an operand that is not a function application is reported as
/// [`BuildError::WindowOperandNotCall`](crate::query::BuildError::WindowOperandNotCall).
///
/// ```
/// use rurqe_core::dialect::{Dialect, SupportsWindowFunctions};
/// use rurqe_core::expr::Expr;
/// use rurqe_core::query::{lag, SelectQuery, Window};
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
/// impl SupportsWindowFunctions for MockDialect {}
///
/// rurqe_core::table!(
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// );
///
/// fn main() {
///     let aggregated = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((
///             OrderUserId,
///             Expr::sum::<OrderUserId>().over(Window::partition_by(OrderUserId)),
///         ))
///         .build()
///         .expect("an aggregate operand keeps one value per group");
///
///     assert!(
///         aggregated.sql.contains("SUM(\"user_id\")"),
///         "{}",
///         aggregated.sql
///     );
///     assert!(
///         aggregated.sql.contains("OVER (PARTITION BY \"user_id\")"),
///         "{}",
///         aggregated.sql
///     );
///
///     let constant = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((
///             OrderUserId,
///             Expr::<i64>::raw("COUNT(*)", Vec::new())
///                 .expect("a template without placeholders")
///                 .over(Window::partition_by(OrderUserId)),
///         ))
///         .build()
///         .expect("a constant operand keeps one value per group");
///
///     assert!(
///         constant.sql.contains("COUNT(*)"),
///         "{}",
///         constant.sql
///     );
///
///     let lagged = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((
///             OrderUserId,
///             lag(OrderUserId).over(Window::order_by_asc(OrderUserId)),
///         ))
///         .build()
///         .expect("a grouped column is a valid window operand");
///
///     assert!(
///         lagged.sql.contains("OVER (ORDER BY \"user_id\" ASC)"),
///         "{}",
///         lagged.sql
///     );
/// }
/// ```
impl<
        GroupByCols,
        T: 'static + Send + Sync,
        Scope: 'static + Send + Sync,
        Cols: 'static + Send + Sync,
        WinD,
        OperandScope,
        OperandAggr: AggregateMarker,
        StmtD,
        Positions,
    > GroupedSelectTarget<GroupByCols, Positions, StmtD>
    for WindowExpr<T, Scope, Cols, WinD, OperandScope, OperandAggr>
where
    Cols: WindowColsInGroupBy<GroupByCols, Positions>,
    WinD: DialectInScope<StmtD>,
{
}

/// Accepts a window expression over a constant operand in a `GROUP BY` query.
///
/// An expression referencing no table — a raw SQL fragment such as
/// `sql!(<i64> "COUNT(*)")` that spells the call itself, or a function call
/// composed of constants — is constant within a group, so a window evaluated over
/// it reads no per-row value and stays projectable from a grouped query even
/// though nothing is aggregated. The pair of the empty operand scope and the
/// [`NonAggregate`] marker records exactly that, and the two implementations stay
/// disjoint because [`NonAggregate`] implements no [`AggregateMarker`].
///
/// The markers describe the grouping behaviour of the operand alone, so the pair
/// also admits a value-shaped operand such as a literal at the type level; the
/// statement build rejects that shape as
/// [`BuildError::WindowOperandNotCall`](crate::query::BuildError::WindowOperandNotCall),
/// because `OVER (...)` attaches to a function application alone.
///
/// The columns the specification partitions by and orders by are checked against
/// the grouped columns exactly like for an aggregate operand.
impl<
        GroupByCols,
        T: 'static + Send + Sync,
        Scope: 'static + Send + Sync,
        Cols: 'static + Send + Sync,
        WinD,
        StmtD,
        Positions,
    > GroupedSelectTarget<GroupByCols, Positions, StmtD>
    for WindowExpr<T, Scope, Cols, WinD, (), NonAggregate>
where
    Cols: WindowColsInGroupBy<GroupByCols, Positions>,
    WinD: DialectInScope<StmtD>,
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
