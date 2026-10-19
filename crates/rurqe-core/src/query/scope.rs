//! Query scopes, joins, and table nullability across relational contexts.

use core::marker::PhantomData;

use crate::schema::{At, ExprScope, Next, Table, TableInScope, TableRef};

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

/// The flavour of join used to bring a table into the query scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinKind {
    /// `INNER JOIN`, which only keeps rows matched in the joined table.
    Inner,
    /// `LEFT JOIN`, which keeps unmatched rows and nulls the joined columns.
    Left,
    /// `CROSS JOIN LATERAL`, which joins every outer row with a correlated subquery.
    CrossLateral,
    /// `LEFT JOIN LATERAL ... ON true`, which keeps outer rows a lateral subquery
    /// leaves unmatched.
    LeftLateral,
}

impl JoinKind {
    /// The SQL text of the join keywords.
    pub const fn as_str(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::CrossLateral => "CROSS JOIN LATERAL",
            JoinKind::LeftLateral => "LEFT JOIN LATERAL",
        }
    }
}

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
