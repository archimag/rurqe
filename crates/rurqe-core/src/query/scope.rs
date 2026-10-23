//! Query scopes, joins, and table nullability across relational contexts.

use core::marker::PhantomData;

use crate::expr::Unscoped;
use crate::schema::{At, ExprScope, Next, Table, TableInScope, TableRef};

// `JoinKind` describes the statement AST shared with the expression module, so
// the canonical definition lives there and is re-exported here.
pub use crate::expr::JoinKind;

/// Marks a table whose columns are never `NULL` in the current relational context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonNullableRel;

/// Marks a table whose columns may be `NULL` because the join may leave it unmatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NullableRel;

/// Resolves the nullability of the table `T` inside the relational scope `Tables`.
#[diagnostic::on_unimplemented(
    message = "table `{T}` is not part of this query's scope",
    label = "add `{T}` to the query before projecting its columns",
    note = "introduce the table with `from`, `inner_join`, `left_join`, or a lateral join"
)]
pub trait TableNullabilityInScope<T: Table, P = At<0>> {
    /// The nullability of `T` contributed by the join that introduced it.
    type Nullability;
}

impl<T: Table> TableNullabilityInScope<T, At<0>> for T {
    type Nullability = NonNullableRel;
}

macro_rules! impl_joined_scope_nullability {
    ($scope:ident => $own:ident) => {
        // Base case: the table the scope was extended with.
        impl<Tables, T: Table> TableNullabilityInScope<T, At<0>> for $scope<Tables, T> {
            type Nullability = $own;
        }

        // Inductive case: tables of the inner scope keep the nullability they had.
        impl<Tables, T: Table, Target: Table, P> TableNullabilityInScope<Target, Next<P>>
            for $scope<Tables, T>
        where
            Tables: TableNullabilityInScope<Target, P>,
        {
            type Nullability = <Tables as TableNullabilityInScope<Target, P>>::Nullability;
        }
    };
}

impl_joined_scope_nullability!(Joined => NonNullableRel);
impl_joined_scope_nullability!(LeftJoined => NullableRel);
impl_joined_scope_nullability!(CrossLateralJoined => NonNullableRel);
impl_joined_scope_nullability!(LeftLateralJoined => NullableRel);

/// The ordered list of tables a query selects from, joined tables included.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid query scope",
    label = "a query must select from a table",
    note = "introduce the first table with `SelectQuery::from` and additional tables with `SelectQuery::inner_join`"
)]
pub trait TableScope {
    /// Appends the table names of the scope in `FROM` order.
    fn push_table_names(out: &mut Vec<&'static str>);

    /// Appends the table references of the scope, aliases included, in `FROM` order.
    fn push_table_refs(out: &mut Vec<TableRef>) {
        let mut names = Vec::new();
        Self::push_table_names(&mut names);
        out.extend(names.into_iter().map(TableRef::plain));
    }
}

impl<T: Table> TableScope for T {
    fn push_table_names(out: &mut Vec<&'static str>) {
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        out.push(T::table_ref());
    }
}

/// Pins the scope positions `P` an expression scope `Self` is checked against.
///
/// [`ExprInScope`](crate::schema::ExprInScope) accepts an expression referencing
/// no table in every scope, so a bare `()` leaves the position `P` ambiguous and
/// the compiler reports `E0282`. This trait names the markers explicitly: the
/// empty scope always sits at [`At<0>`], a single table keeps its own position,
/// and a tuple of tables contributes one position per member.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be checked against a query scope",
    label = "unsupported expression scope",
    note = "expressions may reference `()`, a single table, or a tuple of tables"
)]
pub trait ScopePositions<P = At<0>> {}

impl ScopePositions<At<0>> for () {}

impl ScopePositions<At<0>> for Unscoped {}

impl<T: Table, P> ScopePositions<P> for T {}

macro_rules! impl_scope_positions {
    ($($table:ident $position:ident),+) => {
        impl<$($table: Table),+, $($position),+> ScopePositions<($($position,)+)>
            for ($($table,)+)
        {
        }
    };
}

impl_scope_positions!(T1 P1);
impl_scope_positions!(T1 P1, T2 P2);
impl_scope_positions!(T1 P1, T2 P2, T3 P3);
impl_scope_positions!(T1 P1, T2 P2, T3 P3, T4 P4);
impl_scope_positions!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5);
impl_scope_positions!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6);
impl_scope_positions!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6, T7 P7);
impl_scope_positions!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6, T7 P7, T8 P8);

/// A query scope extending `Tables` with the table `T` joined to it.
pub struct Joined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for Joined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

/// A query scope extending `Tables` with the table `T` joined by a `LEFT JOIN`.
pub struct LeftJoined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for LeftJoined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

impl<Tables: ExprScope, T: Table> ExprScope for Joined<Tables, T> {}

impl<Tables: ExprScope, T: Table> ExprScope for LeftJoined<Tables, T> {}

/// A query scope extending `Tables` with the `LATERAL` subquery exposed as `T`.
pub struct CrossLateralJoined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for CrossLateralJoined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

impl<Tables: ExprScope, T: Table> ExprScope for CrossLateralJoined<Tables, T> {}

/// A query scope extending `Tables` with a `LEFT JOIN LATERAL` subquery exposed as `T`.
pub struct LeftLateralJoined<Tables, T>(PhantomData<fn() -> (Tables, T)>);

impl<Tables: TableScope, T: Table> TableScope for LeftLateralJoined<Tables, T> {
    fn push_table_names(out: &mut Vec<&'static str>) {
        Tables::push_table_names(out);
        out.push(T::NAME);
    }

    fn push_table_refs(out: &mut Vec<TableRef>) {
        Tables::push_table_refs(out);
        out.push(T::table_ref());
    }
}

impl<Tables: ExprScope, T: Table> ExprScope for LeftLateralJoined<Tables, T> {}

macro_rules! impl_joined_scope_table_in_scope {
    ($scope:ident) => {
        // Base case: the table the scope was extended with sits at `At<0>`.
        impl<Tables, T: Table> TableInScope<T, At<0>> for $scope<Tables, T> {}

        // Inductive case: tables of the inner scope sit one position deeper.
        impl<Tables, T: Table, Target: Table, P> TableInScope<Target, Next<P>>
            for $scope<Tables, T>
        where
            Tables: TableInScope<Target, P>,
        {
        }
    };
}

impl_joined_scope_table_in_scope!(Joined);
impl_joined_scope_table_in_scope!(LeftJoined);
impl_joined_scope_table_in_scope!(CrossLateralJoined);
impl_joined_scope_table_in_scope!(LeftLateralJoined);
