//! Projection targets, selections, and nullability lifting for query rows.

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{render_expr, Expr, ExprKind, SqlVal};
use crate::schema::{Column, ColumnBelongsTo, ExprInScope, Table};

use super::scope::{NonNullableRel, NullableRel, ScopePositions, TableNullabilityInScope};

/// A single item projected by a `SELECT` query, such as a column or a window expression.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid projection item",
    label = "must be a column, nullable column, or window expression",
    note = "columns, nullable columns, and window expressions (`.over(...)`) can be projected in a selection"
)]
pub trait SelectTarget {
    /// The Rust type produced for this projection.
    type RustType;

    /// Pushes the identifier name of this target if it is a simple column reference.
    fn push_column_name(&self, out: &mut Vec<&'static str>) {
        let _ = out;
    }

    /// Renders this projection item to the SQL buffer and binds any parameters.
    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>);
}

impl<C: Column> SelectTarget for C {
    type RustType = C::RustType;

    fn push_column_name(&self, out: &mut Vec<&'static str>) {
        out.push(C::NAME);
    }

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        render_expr::<D>(sql, params, &C::expr_kind());
    }
}

impl<T: 'static + Send + Sync, Scope: 'static + Send + Sync> SelectTarget for Expr<T, Scope> {
    type RustType = T;

    fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        render_expr::<D>(sql, params, self.kind());
    }
}

/// Lifts a Rust value type into its nullable form.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no nullable form",
    label = "the column type cannot be lifted into `Option<{Self}>`",
    note = "implement `MakeNullable` for the column type to project it from an outer join"
)]
pub trait MakeNullable {
    /// The nullable representation of the type.
    type Nullable;
}

macro_rules! impl_make_nullable {
    ($($scalar:ty),+) => {
        $(
            impl MakeNullable for $scalar {
                type Nullable = Option<$scalar>;
            }
        )+
    };
}

impl_make_nullable!(i32, i64, f64, bool, String, Vec<u8>);

impl<T> MakeNullable for Option<T> {
    type Nullable = Option<T>;
}

/// Applies the nullability `N` of a relational context to a Rust value type.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be lifted into the nullability of this relational context",
    label = "unsupported column type",
    note = "implement `MakeNullable` for the column type to project it from an outer join"
)]
pub trait ApplyNullability<N> {
    /// The type produced in the relational context `N`.
    type Output;
}

impl<T> ApplyNullability<NonNullableRel> for T {
    type Output = T;
}

impl<T: MakeNullable> ApplyNullability<NullableRel> for T {
    type Output = T::Nullable;
}

/// Derives the effective type of the column `Self` projected from the scope `Tables`.
#[diagnostic::on_unimplemented(
    message = "column `{Self}` cannot be projected from this query's scope",
    label = "the projected column belongs to a table the query does not select from",
    note = "introduce the column's table with `from` or a join before projecting it"
)]
pub trait ColumnInScope<Tables, P> {
    /// The type produced when the column is projected from this scope.
    type Output;
}

impl<Tables, C, P> ColumnInScope<Tables, P> for C
where
    C: Column,
    Tables: TableNullabilityInScope<C::Table, P>,
    C::RustType: ApplyNullability<<Tables as TableNullabilityInScope<C::Table, P>>::Nullability>,
{
    type Output = <C::RustType as ApplyNullability<
        <Tables as TableNullabilityInScope<C::Table, P>>::Nullability,
    >>::Output;
}

/// Derives the type produced by the projection item `Self` in the scope `Tables`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be selected from this query's scope",
    label = "the projected column belongs to a table the query does not select from",
    note = "introduce the column's table with `from` or a join before projecting it"
)]
pub trait SelectTargetInScope<Tables, P> {
    /// The type produced when the item is projected from this scope.
    type Output;
}

impl<Tables, C, P> SelectTargetInScope<Tables, P> for C
where
    C: Column + ColumnInScope<Tables, P>,
{
    type Output = <C as ColumnInScope<Tables, P>>::Output;
}

impl<Tables, T, Scope, P> SelectTargetInScope<Tables, P> for Expr<T, Scope>
where
    Scope: ExprInScope<Tables, P> + ScopePositions<P>,
{
    type Output = T;
}

/// Derives the row type of the selection `Self` projected from the scope `Tables`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be selected from this query's scope",
    label = "one of the projected columns belongs to a table the query does not select from",
    note = "introduce the column's table with `from` or a join before projecting it"
)]
pub trait SelectionFor<Tables, Positions>: Selection {
    /// The row type produced by the selection in this relational context.
    type Row;
}

/// Statically asserts that every projection item of a selection is in the query scope.
pub trait SelectionInScope<Tables, Positions>: Selection {}

impl<Tables, Positions, S> SelectionInScope<Tables, Positions> for S where
    S: SelectionFor<Tables, Positions>
{
}

impl<S: Selection> SelectionFor<(), ()> for S {
    type Row = S::Row;
}

macro_rules! impl_selection_for {
    ($($target:ident $position:ident),+) => {
        impl<Tables, $($target, $position),+> SelectionFor<Tables, ($($position,)+)>
            for ($($target,)+)
        where
            $($target: SelectTarget + SelectTargetInScope<Tables, $position>,)+
        {
            type Row = ($(<$target as SelectTargetInScope<Tables, $position>>::Output,)+);
        }
    };
}

impl_selection_for!(T1 P1);
impl_selection_for!(T1 P1, T2 P2);
impl_selection_for!(T1 P1, T2 P2, T3 P3);
impl_selection_for!(T1 P1, T2 P2, T3 P3, T4 P4);
impl_selection_for!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5);
impl_selection_for!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6);
impl_selection_for!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6, T7 P7);
impl_selection_for!(T1 P1, T2 P2, T3 P3, T4 P4, T5 P5, T6 P6, T7 P7, T8 P8);

/// A list of selected columns whose Rust value types form the query row type.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a supported selection",
    label = "select a column or a tuple of columns",
    note = "selections are column markers written as tuples, such as `(UserId,)` or `(UserId, UserName)`"
)]
pub trait Selection {
    /// The tuple of Rust values produced for a single row.
    type Row;

    /// Appends the SQL names of simple selected columns to `out`.
    fn push_column_names(&self, out: &mut Vec<&'static str>) {
        let _ = out;
    }

    /// Renders all projection items into `sql`, separating them by commas.
    fn render_projections<D: Dialect>(&self, sql: &mut String, _params: &mut Vec<SqlVal>) {
        let mut names = Vec::new();
        self.push_column_names(&mut names);
        for (index, name) in names.into_iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(sql, name);
        }
    }
}

macro_rules! impl_selection {
    ($($target:ident),+) => {
        #[allow(non_snake_case, unused_assignments)]
        impl<$($target: SelectTarget),+> Selection for ($($target,)+) {
            type Row = ($($target::RustType,)+);

            fn push_column_names(&self, out: &mut Vec<&'static str>) {
                let ($($target,)+) = self;
                $($target.push_column_name(out);)+
            }

            fn render_projections<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
                let ($($target,)+) = self;
                let mut first = true;
                $(
                    if !first {
                        sql.push_str(", ");
                    }
                    first = false;
                    $target.render::<D>(sql, params);
                )+
            }
        }
    };
}

impl_selection!(T1);
impl_selection!(T1, T2);
impl_selection!(T1, T2, T3);
impl_selection!(T1, T2, T3, T4);
impl_selection!(T1, T2, T3, T4, T5);
impl_selection!(T1, T2, T3, T4, T5, T6);
impl_selection!(T1, T2, T3, T4, T5, T6, T7);
impl_selection!(T1, T2, T3, T4, T5, T6, T7, T8);

/// Statically asserts that every projected column belongs to the mutated table `T`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a selection of table `{T}`",
    label = "one of the projected columns belongs to another table",
    note = "project only columns of the mutated table in a RETURNING clause or conflict target"
)]
pub trait TableSelection<T: Table>: Selection {}

macro_rules! impl_table_selection {
    ($($column:ident),+) => {
        impl<T: Table, $($column: Column + ColumnBelongsTo<T>),+> TableSelection<T>
            for ($($column,)+)
        {
        }
    };
}

impl_table_selection!(C1);
impl_table_selection!(C1, C2);
impl_table_selection!(C1, C2, C3);
impl_table_selection!(C1, C2, C3, C4);
impl_table_selection!(C1, C2, C3, C4, C5);
impl_table_selection!(C1, C2, C3, C4, C5, C6);
impl_table_selection!(C1, C2, C3, C4, C5, C6, C7);
impl_table_selection!(C1, C2, C3, C4, C5, C6, C7, C8);

/// A column lifted to its nullable form for outer joins.
pub struct Nullable<C>(PhantomData<fn() -> C>);

impl<C: Column> Nullable<C> {
    /// Creates the nullable marker for the column `C`.
    pub fn new() -> Self {
        Nullable(PhantomData)
    }
}

impl<C: Column> Default for Nullable<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Column> Column for Nullable<C> {
    type Table = C::Table;
    type RustType = Option<C::RustType>;

    const NAME: &'static str = C::NAME;

    fn expr_kind() -> ExprKind {
        C::expr_kind()
    }
}
