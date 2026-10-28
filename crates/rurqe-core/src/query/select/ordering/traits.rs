//! What an `ORDER BY` clause of a statement may name.
//!
//! A statement sorts its rows by a column of a relation it selects from, by a
//! computed expression over those relations, or by a window expression evaluated
//! with an `OVER (...)` clause. Both contracts carry the tables the ordering reads
//! and the dialect requirements of the clauses it records, so an ordering that
//! references a table the query does not select from is rejected at the call site
//! instead of rendering a statement the engine cannot resolve.

use crate::dialect::SupportsWindowFunctions;
use crate::expr::{DialectInScope, Expr, ExprKind, NonAggregate};
use crate::query::scope::ScopePositions;
use crate::query::window::WindowExpr;
use crate::schema::{At, Column, ExprInScope, TableInScope};

/// A column, or a tuple of columns, an `ORDER BY` clause sorts by.
///
/// A single column marker orders by that column, while a tuple orders by its
/// columns in declaration order — which is what a keyset cursor needs, because it
/// compares the leading column first and breaks the ties by the columns after it:
///
/// ```text
/// ORDER BY "created_at" DESC, "id" DESC
/// ```
///
/// Every ordered column has to belong to the query scope, so ordering by a column
/// of a table the query does not select from is rejected at the call site:
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
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// rurqe_core::table! {
///     Order => "orders" {
///         OrderId => "id": i64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: `OrderId` belongs to `Order`, which the query does not
///     // select from.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<User>()
///         .select((UserId,))
///         .order_by_asc((OrderId,));
/// }
/// ```
///
/// A statement combined with `UNION`, `INTERSECT`, or `EXCEPT` carries the
/// [`Compound`](crate::query::scope::Compound) scope, which names no relation: such
/// a statement sorts the rows the combination produced, so its ordering names an
/// output column or a position instead of a column of a table in scope, and the
/// table-qualified form is rejected:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::query::{HasProjection, SelectQuery};
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
/// fn base() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
///     SelectQuery::<MockDialect>::new()
///         .from::<User>()
///         .select((UserId,))
/// }
///
/// fn main() {
///     // Fails to compile: a compound statement has no table in scope to order by.
///     let _ = base().union(base()).order_by_asc(UserId);
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid ORDER BY clause",
    label = "order by a column or a tuple of columns",
    note = "ORDER BY accepts column markers or tuples of column markers, such as `UserId` or `(OrderCreatedAt, OrderId)`",
    note = "a compound statement combined with `UNION`, `INTERSECT`, or `EXCEPT` orders its combined rows through `order_by_output` or `order_by_index`, because its output columns no longer belong to a table in scope"
)]
pub trait OrderBy<Tables, Positions = At<0>> {
    /// Appends the ordered column expressions to `out` in declaration order.
    fn push_orderings(&self, out: &mut Vec<ExprKind>);
}

impl<Tables, C, P> OrderBy<Tables, P> for C
where
    C: Column,
    Tables: TableInScope<C::Table, P>,
{
    fn push_orderings(&self, out: &mut Vec<ExprKind>) {
        out.push(C::expr_kind());
    }
}

macro_rules! impl_order_by {
    ($($column:ident $position:ident),+) => {
        impl<Tables, $($column, $position),+> OrderBy<Tables, ($($position,)+)>
            for ($($column,)+)
        where
            $($column: Column, Tables: TableInScope<$column::Table, $position>,)+
        {
            fn push_orderings(&self, out: &mut Vec<ExprKind>) {
                $(out.push($column::expr_kind());)+
            }
        }
    };
}

impl_order_by!(C1 P1);
impl_order_by!(C1 P1, C2 P2);
impl_order_by!(C1 P1, C2 P2, C3 P3);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6, C7 P7);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6, C7 P7, C8 P8);

/// A typed expression an `ORDER BY` clause of a statement sorts by.
///
/// Column markers and tuples of them order through [`OrderBy`], while every
/// other ordering target — a computed expression over the columns of the query
/// scope, a `CASE` expression, a raw SQL fragment, or a window expression
/// evaluated with an `OVER (...)` clause — orders through this trait. Like a
/// projection, the expression carries the tables it reads and the dialect
/// requirements of its clauses, so an ordering that references a table the
/// query does not select from is rejected at the call site instead of
/// rendering a statement the engine cannot resolve:
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
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// rurqe_core::table! {
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: the ordering reads `OrderTotal`, whose table the
///     // query does not select from.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<User>()
///         .select((UserId,))
///         .order_by_expr_asc(Expr::column::<OrderTotal>().gt(0.0));
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid ORDER BY expression",
    label = "order by an expression over the columns of the query scope",
    note = "ORDER BY accepts typed expressions over the query scope, such as `users.age.gt(18)`, or a window expression such as `row_number().over(Window::partition_by(UserId))`",
    note = "the expression may reference only tables the query selects from; order by a bare column or a tuple of columns with `order_by` instead",
    note = "an aggregate collapses the rows of a group, so it orders a grouped statement through `GroupedQuery::order_by_expr` instead"
)]
pub trait OrderExprTarget<Tables, P = At<0>, D = ()> {
    /// Appends the expression of this ordering target to `out`.
    fn into_order_exprs(self, out: &mut Vec<ExprKind>);
}

/// Orders a statement by a computed expression over its query scope.
///
/// The expression keeps the scope it was built with, mirroring the bounds a
/// projection carries: the tables it reads stay checked against the query
/// scope, and its dialect requirements stay checked against the statement
/// dialect.
///
/// The aggregation marker is fixed to
/// [`NonAggregate`](crate::expr::NonAggregate), because a statement that never
/// grouped its rows produces one row per source row and sorts those rows by a
/// per-row value — a bare column reference, an arithmetic combination, a `CASE`
/// expression, or a window expression evaluated with an `OVER (...)` clause. An
/// aggregate collapses the rows of a group, so ordering by one belongs to a
/// grouped statement, where
/// [`GroupedQuery::order_by_expr`](crate::query::GroupedQuery::order_by_expr)
/// takes it. Ordering a plain statement by an aggregate is therefore rejected at
/// the call site instead of rendering a statement the engine refuses:
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
///     }
/// }
///
/// fn main() {
///     // Fails to compile: an aggregate orders the groups of a grouped query,
///     // and this statement never grouped its rows.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .select((OrderUserId,))
///         .order_by_expr_desc(Expr::count_all());
/// }
/// ```
impl<Tables, T, Scope, ExprD, P, D> OrderExprTarget<Tables, P, D>
    for Expr<T, Scope, NonAggregate, ExprD>
where
    T: 'static + Send + Sync,
    Scope: 'static + Send + Sync + ExprInScope<Tables, P> + ScopePositions<P>,
    ExprD: DialectInScope<D>,
{
    fn into_order_exprs(self, out: &mut Vec<ExprKind>) {
        out.push(self.into_kind());
    }
}

/// Orders a statement by a window expression evaluated with an `OVER (...)` clause.
///
/// The statement dialect has to support window functions through
/// [`SupportsWindowFunctions`](crate::dialect::SupportsWindowFunctions), and
/// the window keeps the dialect requirements of its specification, so a
/// specification written with `NULLS FIRST` or `NULLS LAST` orders the
/// statement only through a dialect that renders the modifier.
impl<Tables, T, Scope, Cols, WinD, OperandScope, OperandAggr, P, D> OrderExprTarget<Tables, P, D>
    for WindowExpr<T, Scope, Cols, WinD, OperandScope, OperandAggr>
where
    T: 'static + Send + Sync,
    Scope: 'static + Send + Sync + ExprInScope<Tables, P> + ScopePositions<P>,
    Cols: 'static + Send + Sync,
    WinD: DialectInScope<D>,
    D: SupportsWindowFunctions,
{
    fn into_order_exprs(self, out: &mut Vec<ExprKind>) {
        out.push(self.to_expr_kind());
    }
}
