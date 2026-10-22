//! GROUP BY typestate, grouped-column sets, and the grouped SELECT builder.

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{Aggregate, Expr, ExprKind, NonAggregate};
use crate::schema::{At, Column, ExprInScope, Next, Table, TableInScope};

use super::cte::Cte;
use super::scope::{ScopePositions, TableScope};
use super::selection::{Selection, SelectionFor, SelectTarget};
use super::select::{BuiltQuery, HasProjection, OrderDirection, ProjectionPresent, SelectQuery};
use super::window::WindowExpr;

/// A column or tuple of columns for the `GROUP BY` clause.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid GROUP BY clause",
    label = "group by a column or a tuple of columns",
    note = "group by accepts column markers or tuples of column markers, such as `UserId` or `(UserId, UserName)`"
)]
pub trait GroupBy {
    /// Appends the grouped column expressions to `out` in grouping order.
    fn push_column_names(&self, out: &mut Vec<ExprKind>);
}

impl<C: Column> GroupBy for C {
    fn push_column_names(&self, out: &mut Vec<ExprKind>) {
        out.push(C::expr_kind());
    }
}

macro_rules! impl_group_by {
    ($($column:ident),+) => {
        impl<$($column: Column),+> GroupBy for ($($column,)+) {
            fn push_column_names(&self, out: &mut Vec<ExprKind>) {
                $(out.push($column::expr_kind());)+
            }
        }
    };
}

impl_group_by!(C1);
impl_group_by!(C1, C2);
impl_group_by!(C1, C2, C3);
impl_group_by!(C1, C2, C3, C4);
impl_group_by!(C1, C2, C3, C4, C5);
impl_group_by!(C1, C2, C3, C4, C5, C6);
impl_group_by!(C1, C2, C3, C4, C5, C6, C7);
impl_group_by!(C1, C2, C3, C4, C5, C6, C7, C8);

/// Statically asserts that every grouped column belongs to the query scope.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be grouped in this query's scope",
    label = "the grouped column belongs to a table the query does not select from",
    note = "introduce the column's table with `from` or a join before grouping by it"
)]
pub trait GroupByInScope<Tables, Positions>: GroupBy {}

impl<Tables, C, P> GroupByInScope<Tables, P> for C
where
    C: Column,
    Tables: TableInScope<C::Table, P>,
{
}

macro_rules! impl_group_by_in_scope {
    ($($column:ident $position:ident),+) => {
        impl<Tables, $($column, $position),+> GroupByInScope<Tables, ($($position,)+)>
            for ($($column,)+)
        where
            $($column: Column, Tables: TableInScope<$column::Table, $position>,)+
        {
        }
    };
}

impl_group_by_in_scope!(C1 P1);
impl_group_by_in_scope!(C1 P1, C2 P2);
impl_group_by_in_scope!(C1 P1, C2 P2, C3 P3);
impl_group_by_in_scope!(C1 P1, C2 P2, C3 P3, C4 P4);
impl_group_by_in_scope!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5);
impl_group_by_in_scope!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6);
impl_group_by_in_scope!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6, C7 P7);
impl_group_by_in_scope!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6, C7 P7, C8 P8);

/// The empty base of the type-level set of columns a query groups by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EmptyGroupSet;

/// A type-level set of grouped columns, extended one column at a time.
pub struct GroupSet<Prev, C>(PhantomData<fn() -> (Prev, C)>);

/// Derives the type-level set of grouped columns from a `GROUP BY` clause.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid GROUP BY clause",
    label = "group by a column or a tuple of columns",
    note = "GROUP BY accepts column markers or tuples of column markers, such as `UserId` or `(UserId, UserName)`"
)]
pub trait GroupByColumns<Prev> {
    /// The set of grouped columns, extending `Prev` with the grouped columns.
    type Columns;
}

impl<Prev, C: Column> GroupByColumns<Prev> for C {
    type Columns = GroupSet<Prev, C>;
}

impl<Prev, C1: Column> GroupByColumns<Prev> for (C1,) {
    type Columns = GroupSet<Prev, C1>;
}

impl<Prev, C1: Column, C2: Column> GroupByColumns<Prev> for (C1, C2) {
    type Columns = GroupSet<GroupSet<Prev, C1>, C2>;
}

impl<Prev, C1: Column, C2: Column, C3: Column> GroupByColumns<Prev> for (C1, C2, C3) {
    type Columns = GroupSet<GroupSet<GroupSet<Prev, C1>, C2>, C3>;
}

impl<Prev, C1: Column, C2: Column, C3: Column, C4: Column> GroupByColumns<Prev>
    for (C1, C2, C3, C4)
{
    type Columns = GroupSet<GroupSet<GroupSet<GroupSet<Prev, C1>, C2>, C3>, C4>;
}

impl<Prev, C1: Column, C2: Column, C3: Column, C4: Column, C5: Column> GroupByColumns<Prev>
    for (C1, C2, C3, C4, C5)
{
    type Columns = GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<Prev, C1>, C2>, C3>, C4>, C5>;
}

impl<Prev, C1: Column, C2: Column, C3: Column, C4: Column, C5: Column, C6: Column>
    GroupByColumns<Prev> for (C1, C2, C3, C4, C5, C6)
{
    type Columns =
        GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<Prev, C1>, C2>, C3>, C4>, C5>, C6>;
}

impl<
        Prev,
        C1: Column,
        C2: Column,
        C3: Column,
        C4: Column,
        C5: Column,
        C6: Column,
        C7: Column,
    > GroupByColumns<Prev> for (C1, C2, C3, C4, C5, C6, C7)
{
    type Columns = GroupSet<
        GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<Prev, C1>, C2>, C3>, C4>, C5>, C6>,
        C7,
    >;
}

impl<
        Prev,
        C1: Column,
        C2: Column,
        C3: Column,
        C4: Column,
        C5: Column,
        C6: Column,
        C7: Column,
        C8: Column,
    > GroupByColumns<Prev> for (C1, C2, C3, C4, C5, C6, C7, C8)
{
    type Columns = GroupSet<
        GroupSet<
            GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<GroupSet<Prev, C1>, C2>, C3>, C4>, C5>, C6>,
            C7,
        >,
        C8,
    >;
}

/// Checks that the column `C` is part of the set of grouped columns at position `P`.
#[diagnostic::on_unimplemented(
    message = "column `{C}` cannot be selected in a GROUP BY query because it is neither grouped nor an aggregate",
    label = "the column is not part of the GROUP BY clause",
    note = "add the column to `group_by` or wrap it into an aggregate such as `count`, `sum`, `avg`, `min`, or `max`"
)]
pub trait ColumnInGroupBy<C, P = At<0>> {}

// Base case: the column the set was last extended with sits at `At<0>`.
impl<Prev, C: Column> ColumnInGroupBy<C, At<0>> for GroupSet<Prev, C> {}

// Inductive case: columns of the inner set sit one position deeper.
impl<Prev, C: Column, Target: Column, P> ColumnInGroupBy<Target, Next<P>> for GroupSet<Prev, C>
where
    Prev: ColumnInGroupBy<Target, P>,
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
impl<GroupByCols, T: 'static + Send + Sync, Scope: 'static + Send + Sync>
    GroupedSelectTarget<GroupByCols, At<0>> for Expr<T, Scope, Aggregate>
{
    type Output = T;
}

/// Derives the row item produced by a constant expression in a `GROUP BY` query.
///
/// An expression referencing no table — a literal, a raw SQL fragment such as
/// `sql!(<i64> "COUNT(*)")`, or a comparison between them — is constant within a
/// group, so a grouped query accepts it even though it is not an aggregate.
/// An expression that references a column keeps the scope of its table, so it
/// stays rejected unless that column is grouped.
impl<GroupByCols, T: 'static + Send + Sync> GroupedSelectTarget<GroupByCols, At<0>>
    for Expr<T, (), NonAggregate>
{
    type Output = T;
}

impl<GroupByCols, T: 'static + Send + Sync, Scope: 'static + Send + Sync>
    GroupedSelectTarget<GroupByCols, At<0>> for WindowExpr<T, Scope>
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

impl<D: Dialect, Tables: TableScope, GroupByCols, Projected>
    GroupedQuery<D, Tables, (), GroupByCols, Projected>
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

impl<D: Dialect, Tables, Output, GroupByCols, Projected>
    GroupedQuery<D, Tables, Output, GroupByCols, Projected>
{
    /// Groups the rows further by `columns`, appending them to the grouping.
    pub fn group_by<G, Positions>(
        self,
        columns: G,
    ) -> GroupedQuery<D, Tables, Output, <G as GroupByColumns<GroupByCols>>::Columns, Projected>
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

    /// Restricts grouped rows to those satisfying `predicate`.
    pub fn having<Scope, P>(mut self, predicate: Expr<bool, Scope>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    {
        self.query = self.query.having(predicate);
        self
    }

    /// Appends an ordering criterion for column `col` with `direction`.
    pub fn order_by<C: Column, P>(mut self, col: C, direction: OrderDirection) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.query = self.query.order_by(col, direction);
        self
    }

    /// Appends an ascending ordering criterion for column `col`.
    pub fn order_by_asc<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by(col, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for column `col`.
    pub fn order_by_desc<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by(col, OrderDirection::Desc)
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
    pub fn into_select(self) -> SelectQuery<D, Tables, Output, Projected> {
        self.query
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
}
