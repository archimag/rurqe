//! Column arithmetic: the operators a column marker delegates to its expression.
//!
//! A column marker is a zero-sized value, so the arithmetic of a column is the
//! arithmetic of the expression that column builds. Every operator constructs the
//! column reference with [`Expr::column`](crate::expr::Expr::column) and forwards
//! to the matching operator of [`Expr`](crate::expr::Expr), which keeps a single
//! implementation of the arithmetic algebra instead of two: the accepted operands,
//! the merged scope, the dialect requirements, and the aggregation marker of the
//! result are the ones the expression operators define.
//!
//! Delegating also keeps the scope of the result honest. Which table a column
//! belongs to is declared once, by
//! [`Column::Table`](crate::schema::Column::Table), and the delegation reads that
//! declaration from the column marker instead of letting a caller name the scope
//! it wants: the produced expression references the table of the column, never a
//! table spelled out at the call site.

use crate::schema::alias::{Aliased, AliasedCol, TableAlias};
use crate::schema::table::Column;

/// Implements the arithmetic operators of [`Expr`](crate::expr::Expr) on a
/// column marker.
///
/// A column marker is a zero-sized value, so each operator builds the expression
/// of its column with [`Expr::column`](crate::expr::Expr::column) and delegates to
/// the matching operator of [`Expr`](crate::expr::Expr). The result is therefore
/// exactly what writing the column reference out and applying the operator to it
/// yields: the scopes of the operands merge, the dialect requirements merge, and a
/// column read per row keeps the result a per-row expression.
///
/// The right-hand side is admitted by
/// [`NumericOperand`](crate::expr::NumericOperand) over the Rust type of the
/// column, so an operator accepts a value of that type, a column declared as it,
/// or an expression of it.
///
/// The extra bound the delegation states is the one the expression operator
/// itself requires. A column computes one value per row, so combining it with any
/// operand yields a per-row expression whatever marker the operand carries, and
/// the bound records that the combination of the two resolves to
/// [`NonAggregate`](crate::expr::NonAggregate) for every operand marker — the
/// correspondence the arithmetic of [`Expr`](crate::expr::Expr) derives through
/// [`CombineAggr`](crate::expr::CombineAggr).
///
/// The Rust type of the column is named by a parameter of the implementation
/// instead of being spelled through
/// [`Column::RustType`](crate::schema::Column::RustType) directly. Every column
/// declares the operators — [`table!`](crate::table) invokes this macro for every
/// column it declares — while a predicate stated over the projection of a column
/// whose Rust type is not numeric names one concrete type the compiler knows not
/// to be [numeric](crate::expr::Numeric), and a premise of that shape is checked
/// while the implementation itself is declared. Pinning the type through
/// [`Column::RustType`](crate::schema::Column::RustType) and requiring it to be
/// numeric keeps both requirements premises of the implementation: they are
/// resolved where an operator is used, so the operators exist on every column
/// while their use is accepted exactly for the columns whose Rust type is
/// numeric. The table of the column is pinned the same way: binding
/// [`Column::Table`](crate::schema::Column::Table) to the scope the invocation
/// names lets the delegation reach the expression operator the scope it inherits,
/// so the arithmetic references the table [`Column::Table`](crate::schema::Column::Table)
/// declares instead of one written at the call site.
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
        impl<T, Rhs $($extra_generics)*> ::core::ops::Add<Rhs> for $column
        where
            $($extra_bounds)*
            $column: $crate::schema::Column<RustType = T, Table = $scope>,
            T: $crate::expr::Numeric,
            Rhs: $crate::expr::NumericOperand<T>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
            $crate::expr::NonAggregate: $crate::expr::CombineAggr<
                Rhs::Aggr,
                Rhs::Scope,
                Output = $crate::expr::NonAggregate,
            >,
        {
            type Output = $crate::expr::Expr<
                T,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn add(self, rhs: Rhs) -> Self::Output {
                let column: $crate::expr::Expr<T, $scope> =
                    $crate::expr::Expr::column::<$column>();
                ::core::ops::Add::add(column, rhs)
            }
        }

        impl<T, Rhs $($extra_generics)*> ::core::ops::Sub<Rhs> for $column
        where
            $($extra_bounds)*
            $column: $crate::schema::Column<RustType = T, Table = $scope>,
            T: $crate::expr::Numeric,
            Rhs: $crate::expr::NumericOperand<T>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
            $crate::expr::NonAggregate: $crate::expr::CombineAggr<
                Rhs::Aggr,
                Rhs::Scope,
                Output = $crate::expr::NonAggregate,
            >,
        {
            type Output = $crate::expr::Expr<
                T,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn sub(self, rhs: Rhs) -> Self::Output {
                let column: $crate::expr::Expr<T, $scope> =
                    $crate::expr::Expr::column::<$column>();
                ::core::ops::Sub::sub(column, rhs)
            }
        }

        impl<T, Rhs $($extra_generics)*> ::core::ops::Mul<Rhs> for $column
        where
            $($extra_bounds)*
            $column: $crate::schema::Column<RustType = T, Table = $scope>,
            T: $crate::expr::Numeric,
            Rhs: $crate::expr::NumericOperand<T>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
            $crate::expr::NonAggregate: $crate::expr::CombineAggr<
                Rhs::Aggr,
                Rhs::Scope,
                Output = $crate::expr::NonAggregate,
            >,
        {
            type Output = $crate::expr::Expr<
                T,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn mul(self, rhs: Rhs) -> Self::Output {
                let column: $crate::expr::Expr<T, $scope> =
                    $crate::expr::Expr::column::<$column>();
                ::core::ops::Mul::mul(column, rhs)
            }
        }

        impl<T, Rhs $($extra_generics)*> ::core::ops::Div<Rhs> for $column
        where
            $($extra_bounds)*
            $column: $crate::schema::Column<RustType = T, Table = $scope>,
            T: $crate::expr::Numeric,
            Rhs: $crate::expr::NumericOperand<T>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
            $crate::expr::NonAggregate: $crate::expr::CombineAggr<
                Rhs::Aggr,
                Rhs::Scope,
                Output = $crate::expr::NonAggregate,
            >,
        {
            type Output = $crate::expr::Expr<
                T,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn div(self, rhs: Rhs) -> Self::Output {
                let column: $crate::expr::Expr<T, $scope> =
                    $crate::expr::Expr::column::<$column>();
                ::core::ops::Div::div(column, rhs)
            }
        }

        impl<T, Rhs $($extra_generics)*> ::core::ops::Rem<Rhs> for $column
        where
            $($extra_bounds)*
            $column: $crate::schema::Column<RustType = T, Table = $scope>,
            T: $crate::expr::Numeric,
            Rhs: $crate::expr::NumericOperand<T>,
            $scope: $crate::expr::MergeScope<Rhs::Scope>,
            (): $crate::expr::MergeDialect<Rhs::Dialect>,
            $crate::expr::NonAggregate: $crate::expr::CombineAggr<
                Rhs::Aggr,
                Rhs::Scope,
                Output = $crate::expr::NonAggregate,
            >,
        {
            type Output = $crate::expr::Expr<
                T,
                $crate::expr::Merged<$scope, Rhs::Scope>,
                $crate::expr::NonAggregate,
                $crate::expr::MergedDialect<(), Rhs::Dialect>,
            >;

            fn rem(self, rhs: Rhs) -> Self::Output {
                let column: $crate::expr::Expr<T, $scope> =
                    $crate::expr::Expr::column::<$column>();
                ::core::ops::Rem::rem(column, rhs)
            }
        }
    };
}

// An aliased column keeps the Rust type of the column it addresses, so it takes
// part in arithmetic exactly like the plain marker does, and its expression
// stays qualified by the alias. The scope named here is the table the column
// addresses through the alias, which is what `Column::Table` of an aliased marker
// resolves to. The invocation follows the helper definition so that the macro is
// in scope wherever the aliased marker is used.
__rurqe_column_arithmetic!(
    [, A, C]
    [A: TableAlias, C: Column,]
    AliasedCol<A, C>,
    Aliased<C::Table, A>
);
