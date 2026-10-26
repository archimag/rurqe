//! Ordering, pagination, and the clause-by-clause recording of both.

use crate::dialect::{Dialect, SupportsNullsOrdering, SupportsWindowFunctions};
use crate::expr::{
    DialectInScope, Expr, ExprKind, NonAggregate, NullsOrder, OrderCriterion, OrderDirection,
};
use crate::query::scope::{Compound, ScopePositions};
use crate::query::window::WindowExpr;
use crate::schema::{At, Column, ExprInScope, Table, TableInScope};

use super::{HasProjection, SelectQuery, SetOpApplied, TailClosed};

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
/// [`Compound`] scope, which names no relation: such a statement sorts the rows
/// the combination produced, so its ordering names an output column or a position
/// instead of a column of a table in scope, and the table-qualified form is
/// rejected:
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
impl<Tables, T, Scope, Cols, WinD, OperandScope, OperandAggr, P, D>
    OrderExprTarget<Tables, P, D>
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

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
    /// Appends an ordering criterion for `columns` with `direction`.
    ///
    /// `columns` is a column marker or a tuple of them, so a keyset cursor is
    /// ordered as one clause — `.order_by_desc((OrderCreatedAt, OrderId))` sorts by
    /// the timestamp first and breaks the ties by the identifier:
    ///
    /// ```text
    /// ORDER BY "created_at" DESC, "id" DESC
    /// ```
    ///
    /// An ordering written before a set operation belongs to the left query and
    /// is rendered inside its parentheses; an ordering written after one applies
    /// to the combined rows.
    pub fn order_by<O, P>(mut self, columns: O, direction: OrderDirection) -> Self
    where
        O: OrderBy<Tables, P>,
    {
        let mut expressions = Vec::new();
        columns.push_orderings(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction));
        }
        self
    }

    /// Appends an ordering criterion fixing where the NULL values of `columns` sort.
    pub fn order_by_nulls<O, P>(
        mut self,
        columns: O,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        let mut expressions = Vec::new();
        columns.push_orderings(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction).nulls(nulls));
        }
        self
    }

    /// Appends an ascending ordering criterion for `columns`.
    pub fn order_by_asc<O, P>(self, columns: O) -> Self
    where
        O: OrderBy<Tables, P>,
    {
        self.order_by(columns, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for `columns`.
    pub fn order_by_desc<O, P>(self, columns: O) -> Self
    where
        O: OrderBy<Tables, P>,
    {
        self.order_by(columns, OrderDirection::Desc)
    }

    /// Appends an ascending ordering criterion putting NULL values first.
    pub fn order_by_asc_nulls_first<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion putting NULL values last.
    pub fn order_by_asc_nulls_last<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion putting NULL values first.
    pub fn order_by_desc_nulls_first<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion putting NULL values last.
    pub fn order_by_desc_nulls_last<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Desc, NullsOrder::Last)
    }

    /// Appends an ordering criterion for `expression` with `direction`.
    ///
    /// The criterion renders the expression instead of a bare column, so a
    /// computed value — an arithmetic expression over the columns of the query
    /// scope, a `CASE` expression, or a window expression — sorts the rows it
    /// evaluates:
    ///
    /// ```text
    /// ORDER BY "total" * "quantity" DESC
    /// ```
    ///
    /// The criterion carries a per-row expression, so the statement sorts the
    /// rows it produced without grouping them first: an aggregate collapses the
    /// rows of a group and orders a grouped statement through
    /// [`GroupedQuery::order_by_expr`](crate::query::GroupedQuery::order_by_expr)
    /// instead, which is why passing one here is rejected at the call site.
    ///
    /// The expression is checked against the query scope like a projection,
    /// so an ordering that references a table the query does not select from
    /// is rejected at the call site:
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
    ///         .select((OrderUserId,))
    ///         .order_by_expr_desc(Expr::column::<OrderTotal>())
    ///         .build()
    ///         .expect("valid query");
    ///
    ///     assert_eq!(
    ///         built.sql,
    ///         "SELECT \"user_id\" FROM \"orders\" ORDER BY \"total\" DESC"
    ///     );
    /// }
    /// ```
    ///
    /// A window expression orders the statement through the same builder,
    /// keeping its `OVER (...)` specification inside the criterion:
    ///
    /// ```
    /// use rurqe_core::dialect::{Dialect, SupportsWindowFunctions};
    /// use rurqe_core::query::{SelectQuery, Window, row_number};
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .select((OrderUserId,))
    ///         .order_by_expr_desc(row_number().over(Window::partition_by(OrderUserId)))
    ///         .build()
    ///         .expect("valid query");
    ///
    ///     assert!(
    ///         built.sql.contains("OVER"),
    ///         "window specification rendered: {}",
    ///         built.sql
    ///     );
    /// }
    /// ```
    pub fn order_by_expr<E, P>(mut self, expression: E, direction: OrderDirection) -> Self
    where
        E: OrderExprTarget<Tables, P, D>,
    {
        let mut expressions = Vec::new();
        expression.into_order_exprs(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction));
        }
        self
    }

    /// Appends an ascending ordering criterion for `expression`.
    pub fn order_by_expr_asc<E, P>(self, expression: E) -> Self
    where
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr(expression, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for `expression`.
    pub fn order_by_expr_desc<E, P>(self, expression: E) -> Self
    where
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr(expression, OrderDirection::Desc)
    }

    /// Appends an ordering criterion for `expression` fixing where its NULL
    /// values sort.
    pub fn order_by_expr_nulls<E, P>(
        mut self,
        expression: E,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        let mut expressions = Vec::new();
        expression.into_order_exprs(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction).nulls(nulls));
        }
        self
    }

    /// Appends an ascending ordering criterion for `expression` putting NULL
    /// values first.
    pub fn order_by_expr_asc_nulls_first<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion for `expression` putting NULL
    /// values last.
    pub fn order_by_expr_asc_nulls_last<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion for `expression` putting NULL
    /// values first.
    pub fn order_by_expr_desc_nulls_first<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion for `expression` putting NULL
    /// values last.
    pub fn order_by_expr_desc_nulls_last<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Desc, NullsOrder::Last)
    }

    /// Records `criterion` on the clause the query currently orders with.
    ///
    /// An ordering written after a set operation belongs to the combined rows
    /// rather than to the left query, so it is recorded on the compound statement
    /// and rendered against the output column names of the projections.
    pub(crate) fn push_ordering(&mut self, criterion: OrderCriterion) {
        if self.set_operations.is_empty() {
            self.order_by.push(criterion);
        } else {
            self.compound_order_by.push(criterion);
        }
    }

    /// Limits the number of returned rows to `limit`.
    ///
    /// A limit written before a set operation belongs to the left query; a limit
    /// written after one applies to the combined rows.
    pub fn limit(mut self, limit: u32) -> Self {
        if self.set_operations.is_empty() {
            self.limit = Some(limit);
        } else {
            self.compound_limit = Some(limit);
        }
        self
    }

    /// Skips the first `offset` rows of the result.
    ///
    /// An offset written before a set operation belongs to the left query; an
    /// offset written after one applies to the combined rows.
    pub fn offset(mut self, offset: u32) -> Self {
        if self.set_operations.is_empty() {
            self.offset = Some(offset);
        } else {
            self.compound_offset = Some(offset);
        }
        self
    }
}

/// Builds the `ORDER BY` node naming the 1-based position `index` of a compound result.
///
/// A compound statement sorts the rows its projections produced, so the criterion
/// names an output column rather than a column of the statements it combines. The
/// position renders as a bare number, and the pre-flight validation of the
/// assembled statement checks it against the projection list.
fn compound_order_position(index: u32) -> ExprKind {
    ExprKind::Raw {
        fragments: vec![index.to_string()],
        params: Vec::new(),
    }
}

/// Builds the `ORDER BY` node naming the output column `C` of a compound result.
///
/// The node carries the name of `C` without the qualifier its own marker renders:
/// a compound statement sorts the columns its projection list produced, so a
/// table-qualified reference would name a relation the combined statement no
/// longer declares and is rejected by the engine.
fn compound_order_column<C: Column>() -> ExprKind {
    ExprKind::Column {
        table: <C::Table as Table>::NAME,
        name: C::NAME,
    }
}

/// Implements the ordering builders of a compound statement for one tail state.
///
/// The builders are reachable while the ordering of the combined rows is still
/// being written: from [`SetOpApplied`], where the last set operation left the
/// query, and from [`TailClosed`], where the previous criterion left it. Every call
/// moves the query into [`TailClosed`], so a criterion can never be written after
/// another operand was combined with the statement.
macro_rules! impl_compound_ordering {
    ($lock:ty) => {
        impl<D: Dialect, Output> SelectQuery<D, Compound, Output, HasProjection, $lock> {
            /// Orders the combined rows by the 1-based position `index` of the
            /// projection list.
            ///
            /// ```text
            /// SELECT "id", "name" FROM "users" UNION SELECT "id", "name" FROM "archive"
            ///     ORDER BY 2 DESC
            /// ```
            ///
            /// The position is checked against the projection list when the
            /// statement is built, so an out-of-range index is reported as
            /// [`BuildError::CompoundOrderIndex`](crate::query::BuildError::CompoundOrderIndex)
            /// instead of reaching the engine.
            pub fn order_by_index(
                mut self,
                index: u32,
                direction: OrderDirection,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.push_ordering(OrderCriterion::new(compound_order_position(index), direction));
                self.tail_closed()
            }

            /// Orders the combined rows by the 1-based position `index`, ascending.
            pub fn order_by_index_asc(
                self,
                index: u32,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_index(index, OrderDirection::Asc)
            }

            /// Orders the combined rows by the 1-based position `index`, descending.
            pub fn order_by_index_desc(
                self,
                index: u32,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_index(index, OrderDirection::Desc)
            }

            /// Orders the combined rows by their `column` output column.
            ///
            /// ```text
            /// SELECT "id", "total" FROM "orders" UNION SELECT "id", "total" FROM "archive"
            ///     ORDER BY "total" ASC
            /// ```
            ///
            /// The column is rendered by its bare name, and the pre-flight
            /// validation of the assembled statement checks that the projection list
            /// produces it — reported as
            /// [`BuildError::CompoundOrderColumn`](crate::query::BuildError::CompoundOrderColumn)
            /// otherwise. An expression projected without an alias produces no output
            /// name, so such a column is ordered by its position instead.
            pub fn order_by_output<C: Column>(
                mut self,
                _column: C,
                direction: OrderDirection,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.push_ordering(OrderCriterion::new(compound_order_column::<C>(), direction));
                self.tail_closed()
            }

            /// Orders the combined rows by their `column` output column, ascending.
            pub fn order_by_output_asc<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_output(column, OrderDirection::Asc)
            }

            /// Orders the combined rows by their `column` output column, descending.
            pub fn order_by_output_desc<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_output(column, OrderDirection::Desc)
            }

            /// Orders the combined rows by their `column` output column, fixing where
            /// its NULL values sort.
            pub fn order_by_output_nulls<C: Column>(
                mut self,
                _column: C,
                direction: OrderDirection,
                nulls: NullsOrder,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.push_ordering(
                    OrderCriterion::new(compound_order_column::<C>(), direction).nulls(nulls),
                );
                self.tail_closed()
            }

            /// Orders the combined rows by their `column` output column, ascending,
            /// putting NULL values first.
            pub fn order_by_output_asc_nulls_first<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Asc, NullsOrder::First)
            }

            /// Orders the combined rows by their `column` output column, ascending,
            /// putting NULL values last.
            pub fn order_by_output_asc_nulls_last<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Asc, NullsOrder::Last)
            }

            /// Orders the combined rows by their `column` output column, descending,
            /// putting NULL values first.
            pub fn order_by_output_desc_nulls_first<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Desc, NullsOrder::First)
            }

            /// Orders the combined rows by their `column` output column, descending,
            /// putting NULL values last.
            pub fn order_by_output_desc_nulls_last<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Desc, NullsOrder::Last)
            }
        }
    };
}

impl_compound_ordering!(SetOpApplied);
impl_compound_ordering!(TailClosed);
