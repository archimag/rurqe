//! Type-level sets of grouped columns and the compile-time membership checks.
//!
//! A `GROUP BY` clause records its columns twice: as rendered expression nodes
//! and as a type-level [`GroupSet`], one node per grouped column. The set is what
//! lets a grouped query prove at the call site that every projected column, every
//! column a window reads, and every `HAVING` predicate reads a column the query
//! groups by, so a statement the database would reject never reaches the renderer.

use core::marker::PhantomData;

use crate::expr::ExprKind;
use crate::schema::{At, Column, Next, TableInScope};

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

/// Folds a type-level set of columns onto another set.
///
/// A window specification records the columns it partitions by and orders by, one
/// [`GroupSet`] node per column. A window expression merges that set with its own
/// columns, so the trait rewrites the appended set on top of the set it is merged
/// into while keeping the order the columns were declared in.
pub trait GroupSetColumns<Prev> {
    /// The set holding the columns of `Prev` followed by the columns of `Self`.
    type Columns;
}

// The empty appended set leaves the set it is merged into untouched.
impl<Prev> GroupSetColumns<Prev> for EmptyGroupSet {
    type Columns = Prev;
}

// The appended set contributes its last column on top of the columns folded in so
// far, so a tuple of window columns keeps its declaration order.
impl<Prev, Inner, C: Column> GroupSetColumns<Prev> for GroupSet<Inner, C>
where
    Inner: GroupSetColumns<Prev>,
{
    type Columns = GroupSet<<Inner as GroupSetColumns<Prev>>::Columns, C>;
}

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

/// Checks that every column a window expression reads is part of the grouped columns.
///
/// A window function is evaluated over the rows of a group once the rows are
/// grouped, so the columns its specification partitions by and orders by have to
/// survive the grouping as well: the database evaluates `PARTITION BY` and
/// `ORDER BY` after `GROUP BY` and cannot read a column the grouping dropped.
///
/// The `Positions` parameter records where each window column sits in the
/// `GROUP BY` clause, one entry per column, so the columns are checked one
/// position at a time and a window reading an ungrouped column is rejected:
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
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: the window orders by `total`, which the query does not
///     // group by.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((OrderUserId, row_number().over(Window::order_by_asc(OrderTotal))));
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "a window expression of this GROUP BY query reads a column the query does not group by",
    label = "the window partitions or orders by a column outside the GROUP BY clause",
    note = "add the column to `group_by`, or partition and order the window by a column the query already groups by"
)]
pub trait WindowColsInGroupBy<GroupByCols, Positions = ()> {}

// A window referencing no column reads nothing that would have to be grouped.
impl<GroupByCols> WindowColsInGroupBy<GroupByCols, ()> for EmptyGroupSet {}

// The column the window declared last must sit at the recorded position of the
// grouping, and the columns declared before it are checked one level deeper.
impl<GroupByCols, Prev, C, Head, Tail> WindowColsInGroupBy<GroupByCols, (Head, Tail)>
    for GroupSet<Prev, C>
where
    C: Column,
    GroupByCols: ColumnInGroupBy<C, Head>,
    Prev: WindowColsInGroupBy<GroupByCols, Tail>,
{
}
