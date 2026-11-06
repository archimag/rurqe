//! Static compatibility between a column type and a value stored in it.

use crate::expr::builder::Expr;

/// Statically asserts that a value of type `ValType` may be stored in a column
/// whose Rust type is `ColType`.
///
/// The trait types the DML assignment contract: `Assignment::set` requires
/// `V: SqlCompatible<C::RustType, V> + Into<SqlVal>`, so a value of an unrelated
/// type cannot reach a `SET` clause or an `INSERT` row. Beside the exact type
/// match it admits the borrowed forms a caller naturally holds — a string literal
/// for a `String` column, a byte slice for a `Vec<u8>` column — together with the
/// expressions an assignment may store.
///
/// Comparison operands are checked by [`IntoExpr`](crate::expr::IntoExpr)
/// instead, so the two contracts stay orthogonal.
#[diagnostic::on_unimplemented(
    message = "value of type `{ValType}` cannot be stored in a column of type `{ColType}`",
    label = "incompatible value type",
    note = "convert the value to the column's `Column::RustType` before assigning it"
)]
pub trait SqlCompatible<ColType, ValType> {}

impl<T> SqlCompatible<T, T> for T {}

impl<'a> SqlCompatible<String, &'a str> for &'a str {}

impl<'a> SqlCompatible<Vec<u8>, &'a [u8]> for &'a [u8] {}

impl<T, Scope, Aggr, D> SqlCompatible<T, Expr<T, Scope, Aggr, D>> for Expr<T, Scope, Aggr, D> {}

#[cfg(feature = "decimal")]
impl SqlCompatible<rust_decimal::Decimal, &rust_decimal::Decimal> for &rust_decimal::Decimal {}

#[cfg(feature = "uuid")]
impl<'a> SqlCompatible<uuid::Uuid, &'a str> for &'a str {}

#[cfg(feature = "uuid")]
impl<'a> SqlCompatible<uuid::Uuid, &'a uuid::Uuid> for &'a uuid::Uuid {}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::NaiveDateTime, &chrono::NaiveDateTime> for &chrono::NaiveDateTime {}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::DateTime<chrono::Utc>, &chrono::DateTime<chrono::Utc>>
    for &chrono::DateTime<chrono::Utc>
{
}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::NaiveDate, &chrono::NaiveDate> for &chrono::NaiveDate {}

#[cfg(feature = "serde_json")]
impl SqlCompatible<serde_json::Value, &serde_json::Value> for &serde_json::Value {}

#[cfg(feature = "serde_json")]
impl<'a, T: serde::Serialize> SqlCompatible<crate::expr::Json<T>, &'a crate::expr::Json<T>>
    for &'a crate::expr::Json<T>
{
}
