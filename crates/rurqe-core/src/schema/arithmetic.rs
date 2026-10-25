//! Column arithmetic: the operators a column marker composes with SQL values.

use crate::expr::{BinaryOp, ExprKind, Numeric, NumericOperand};
use crate::schema::alias::{Aliased, AliasedCol, TableAlias};
use crate::schema::table::Column;

/// A value an arithmetic operator of a column marker combines with.
///
/// The trait is implemented for every operand the column's
/// [`RustType`](Column::RustType) accepts, so a column marker carries the
/// arithmetic operators exactly when its Rust type is a
/// [numeric](crate::expr::Numeric) type: `i32`, `i64`, `f64`, or an exact
/// [`rust_decimal::Decimal`].
#[diagnostic::on_unimplemented(
    message = "column `{C}` does not support SQL arithmetic with `{Self}`",
    label = "the column's Rust type is not numeric, or the value has another type",
    note = "arithmetic combines a numeric column with a value or an expression of the same Rust type"
)]
#[doc(hidden)]
pub trait ColumnArithmetic<C: Column> {
    /// The scope of the tables the operand references.
    type Scope;

    /// The aggregation marker the operand contributes.
    type Aggr;

    /// The dialect marker of the operand.
    type Dialect;

    /// Converts `self` into the expression node of the operand.
    fn into_operand_kind(self) -> ExprKind;
}

impl<C, V> ColumnArithmetic<C> for V
where
    C: Column,
    C::RustType: Numeric,
    V: NumericOperand<C::RustType>,
{
    type Scope = V::Scope;
    type Aggr = V::Aggr;
    type Dialect = V::Dialect;

    fn into_operand_kind(self) -> ExprKind {
        <V as NumericOperand<C::RustType>>::into_operand_kind(self)
    }
}

/// Assembles the binary expression node of a column arithmetic operator.
///
/// The node is built here instead of in the macro expansion, so the arithmetic
/// of a column marker keeps using the tree the expression builders use.
#[doc(hidden)]
pub fn arithmetic_kind(op: BinaryOp, lhs: ExprKind, rhs: ExprKind) -> ExprKind {
    ExprKind::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

/// Implements the arithmetic operators of [`Expr`](crate::expr::Expr) on a
/// column marker.
///
/// A column marker is a zero-sized value, so every operator builds the
/// expression of its column and yields the expression the matching operator of
/// [`Expr`](crate::expr::Expr) yields: the scopes of the operands merge, and a
/// column read per row keeps the result a per-row expression. The operators
/// exist exactly for the columns whose Rust type is numeric, because the
/// [operand](ColumnArithmetic) only has an implementation for such a type.
///
/// Because every operand of a numeric column computes one value per row, the
/// output names the [`NonAggregate`](crate::expr::NonAggregate) marker directly
/// — the marker every [`CombineAggr`](crate::expr::CombineAggr) projection from
/// `NonAggregate` resolves to — which also keeps the operator bodies on
/// `Expr::from_kind`, the constructor that builds per-row expressions only.
///
/// The two bracket groups carry the extra generics and the extra bounds of a
/// marker parameterised over a table alias, and stay empty for the markers
/// [`table!`](crate::table) declares.
#[doc(hidden)]
#[macro_export]
macro_rules! __rurqe_column_arithmetic {
    (
        [$($extra_generics:tt)*]
        [$($extra_bounds:tt)*]
        $column:ty,
        $scope:ty $(,)?
    ) => {
        impl<Rhs $($extra_generics)*> ::core::ops::Add<Rhs> for $column
        where
            $($extra_bounds)*
            Rhs: $crate::schema::ColumnArithmetic<$column>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
        {
            type Output = $crate::expr::Expr<
                <$column as $crate::schema::Column>::RustType,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn add(self, rhs: Rhs) -> Self::Output {
                $crate::expr::Expr::from_kind($crate::schema::arithmetic_kind(
                    $crate::expr::BinaryOp::Add,
                    <$column as $crate::schema::Column>::expr_kind(),
                    <Rhs as $crate::schema::ColumnArithmetic<$column>>::into_operand_kind(rhs),
                ))
            }
        }

        impl<Rhs $($extra_generics)*> ::core::ops::Sub<Rhs> for $column
        where
            $($extra_bounds)*
            Rhs: $crate::schema::ColumnArithmetic<$column>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
        {
            type Output = $crate::expr::Expr<
                <$column as $crate::schema::Column>::RustType,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn sub(self, rhs: Rhs) -> Self::Output {
                $crate::expr::Expr::from_kind($crate::schema::arithmetic_kind(
                    $crate::expr::BinaryOp::Sub,
                    <$column as $crate::schema::Column>::expr_kind(),
                    <Rhs as $crate::schema::ColumnArithmetic<$column>>::into_operand_kind(rhs),
                ))
            }
        }

        impl<Rhs $($extra_generics)*> ::core::ops::Mul<Rhs> for $column
        where
            $($extra_bounds)*
            Rhs: $crate::schema::ColumnArithmetic<$column>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
        {
            type Output = $crate::expr::Expr<
                <$column as $crate::schema::Column>::RustType,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn mul(self, rhs: Rhs) -> Self::Output {
                $crate::expr::Expr::from_kind($crate::schema::arithmetic_kind(
                    $crate::expr::BinaryOp::Mul,
                    <$column as $crate::schema::Column>::expr_kind(),
                    <Rhs as $crate::schema::ColumnArithmetic<$column>>::into_operand_kind(rhs),
                ))
            }
        }

        impl<Rhs $($extra_generics)*> ::core::ops::Div<Rhs> for $column
        where
            $($extra_bounds)*
            Rhs: $crate::schema::ColumnArithmetic<$column>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
        {
            type Output = $crate::expr::Expr<
                <$column as $crate::schema::Column>::RustType,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn div(self, rhs: Rhs) -> Self::Output {
                $crate::expr::Expr::from_kind($crate::schema::arithmetic_kind(
                    $crate::expr::BinaryOp::Div,
                    <$column as $crate::schema::Column>::expr_kind(),
                    <Rhs as $crate::schema::ColumnArithmetic<$column>>::into_operand_kind(rhs),
                ))
            }
        }

        impl<Rhs $($extra_generics)*> ::core::ops::Rem<Rhs> for $column
        where
            $($extra_bounds)*
            Rhs: $crate::schema::ColumnArithmetic<$column>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
        {
            type Output = $crate::expr::Expr<
                <$column as $crate::schema::Column>::RustType,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn rem(self, rhs: Rhs) -> Self::Output {
                $crate::expr::Expr::from_kind($crate::schema::arithmetic_kind(
                    $crate::expr::BinaryOp::Rem,
                    <$column as $crate::schema::Column>::expr_kind(),
                    <Rhs as $crate::schema::ColumnArithmetic<$column>>::into_operand_kind(rhs),
                ))
            }
        }
    };
}

// An aliased column keeps the Rust type of the column it addresses, so it takes
// part in arithmetic exactly like the plain marker does, and its expression
// stays qualified by the alias. The invocation follows the helper definition so
// that the macro is in scope wherever the aliased marker is used.
__rurqe_column_arithmetic!(
    [, A, C]
    [A: TableAlias, C: Column,]
    AliasedCol<A, C>,
    Aliased<C::Table, A>
);
