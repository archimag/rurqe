//! Free-function helpers building aggregate, window, and scalar expressions.

use super::ast::ExprKind;
use super::builder::{Aggregate, Expr};
use super::ops::IntoExpr;
use crate::schema::Column;

/// Builds a `COUNT(*)` expression over the rows of the query.
///
/// The produced expression carries the [`Aggregate`] marker, so it stays
/// projectable from a grouped query.
pub fn count() -> Expr<i64, (), Aggregate> {
    Expr::<i64>::count_all()
}

/// Builds a `COUNT(*)` expression over the rows of the query.
///
/// The produced expression carries the [`Aggregate`] marker, so it stays
/// projectable from a grouped query.
pub fn count_all() -> Expr<i64, (), Aggregate> {
    Expr::<i64>::count_all()
}

/// Builds a `SUM(col)` expression for the column `col`.
///
/// The produced expression carries the [`Aggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped.
pub fn sum<C: Column>(_col: C) -> Expr<C::RustType, (), Aggregate> {
    Expr::<C::RustType>::sum::<C>()
}

/// Builds an `AVG(col)` expression for the column `col`.
///
/// The produced expression carries the [`Aggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped.
pub fn avg<C: Column>(_col: C) -> Expr<f64, (), Aggregate> {
    Expr::<f64>::avg::<C>()
}

/// Builds a `MIN(col)` expression for the column `col`.
///
/// The produced expression carries the [`Aggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped.
pub fn min<C: Column>(_col: C) -> Expr<C::RustType, (), Aggregate> {
    Expr::<C::RustType>::min::<C>()
}

/// Builds a `MAX(col)` expression for the column `col`.
///
/// The produced expression carries the [`Aggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped.
pub fn max<C: Column>(_col: C) -> Expr<C::RustType, (), Aggregate> {
    Expr::<C::RustType>::max::<C>()
}

/// Builds a `COALESCE(col, fallback)` expression for the column `col`.
///
/// The fallback is bound as a parameter, so it must be compatible with the
/// column's Rust type.
pub fn coalesce<C, V>(_col: C, fallback: V) -> Expr<C::RustType>
where
    C: Column,
    V: IntoExpr<C::RustType>,
{
    Expr::new(ExprKind::Function {
        name: "COALESCE",
        args: vec![ExprKind::Column { name: C::NAME }, fallback.into_expr_kind()],
    })
}
