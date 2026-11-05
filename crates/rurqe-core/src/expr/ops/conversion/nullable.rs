//! Comparisons of a nullable column against a value of its inner type.
//!
//! A column declared `Option<T>` reports `Option<T>` as its
//! [`RustType`](crate::schema::Column::RustType), while the value a caller holds
//! is a `T`. Comparing the column with that value is the ordinary SQL `=`,
//! `<>`, `BETWEEN`, or `IN` over a nullable attribute: the predicate matches the
//! rows whose value equals the operand and no row storing `NULL`, because SQL
//! compares an absent value as unknown rather than as a value equal or unequal
//! to the operand.
//!
//! The implementations below therefore lift each value form into its optional
//! counterpart, and each one delegates to the implementation of the inner type.
//! A nullable comparison builds the node its inner type builds, so the encoding
//! of a value exists once.
//!
//! An `Option<T>` value is deliberately not an operand form. Binding `None` as a
//! parameter would compare the column with `NULL` — a predicate that is never
//! true — and deriving the operator from a runtime value would let the value
//! decide which clause the statement carries. Absence is stated by
//! [`Expr::is_null`](crate::expr::Expr::is_null) and
//! [`Expr::is_not_null`](crate::expr::Expr::is_not_null), which name the SQL
//! operator in the statement itself.

use crate::expr::ast::ExprKind;
use crate::expr::builder::NonAggregate;

use super::IntoExpr;

/// Declares the optional operand form of every value form listed.
///
/// The body delegates to the implementation of the inner type, so a value is
/// encoded exactly as it is encoded for a non-nullable column.
macro_rules! nullable_value_operands {
    ($($(#[$meta:meta])* $value:ty => $inner:ty,)+) => {
        $(
            $(#[$meta])*
            impl IntoExpr<Option<$inner>> for $value {
                type Scope = ();
                type Aggr = NonAggregate;
                type Dialect = ();

                fn into_expr_kind(self) -> ExprKind {
                    <$value as IntoExpr<$inner>>::into_expr_kind(self)
                }
            }
        )+
    };
}

nullable_value_operands! {
    i32 => i32,
    i64 => i64,
    f64 => f64,
    bool => bool,
    String => String,
    &str => String,
    Vec<u8> => Vec<u8>,
    &[u8] => Vec<u8>,
}

#[cfg(feature = "decimal")]
nullable_value_operands! {
    rust_decimal::Decimal => rust_decimal::Decimal,
    &rust_decimal::Decimal => rust_decimal::Decimal,
}

#[cfg(feature = "uuid")]
nullable_value_operands! {
    uuid::Uuid => uuid::Uuid,
    &uuid::Uuid => uuid::Uuid,
    &str => uuid::Uuid,
}

#[cfg(feature = "chrono")]
nullable_value_operands! {
    chrono::NaiveDateTime => chrono::NaiveDateTime,
    &chrono::NaiveDateTime => chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc> => chrono::DateTime<chrono::Utc>,
    &chrono::DateTime<chrono::Utc> => chrono::DateTime<chrono::Utc>,
    chrono::NaiveDate => chrono::NaiveDate,
    &chrono::NaiveDate => chrono::NaiveDate,
}

#[cfg(feature = "serde_json")]
nullable_value_operands! {
    serde_json::Value => serde_json::Value,
    &serde_json::Value => serde_json::Value,
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<Option<crate::expr::Json<T>>> for crate::expr::Json<T> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        <crate::expr::Json<T> as IntoExpr<crate::expr::Json<T>>>::into_expr_kind(self)
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<Option<crate::expr::Json<T>>> for &crate::expr::Json<T> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        <&crate::expr::Json<T> as IntoExpr<crate::expr::Json<T>>>::into_expr_kind(self)
    }
}
