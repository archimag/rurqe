//! Projection rules of window expressions in a grouped query.
//!
//! A window function is evaluated once the rows are grouped, so the columns its
//! specification names and the operand it evaluates over have to survive the
//! grouping. The two implementations below state that for an aggregating operand
//! and for a constant operand; the contract they implement lives in the parent
//! module as `GroupedSelectTarget`.

use crate::expr::{AggregateMarker, DialectInScope, NonAggregate};

use super::super::window::WindowExpr;
use super::{GroupedSelectTarget, WindowColsInGroupBy};

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
