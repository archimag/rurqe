//! Static compatibility between a compared column type and an operand value.

use crate::dialect::Dialect;
use crate::query::{HasProjection, SelectQuery, TableScope};

use crate::expr::builder::Expr;

/// Statically asserts that a value of type `ValType` may be compared with a
/// column whose Rust type is `ColType`.
///
/// The trait is satisfied when both types match exactly, and additionally for
/// string literals compared with string columns.
#[diagnostic::on_unimplemented(
    message = "value of type `{ValType}` cannot be compared with a column of type `{ColType}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait SqlCompatible<ColType, ValType> {}

impl<T> SqlCompatible<T, T> for T {}

impl<'a> SqlCompatible<String, &'a str> for &'a str {}

impl<'a> SqlCompatible<Vec<u8>, &'a [u8]> for &'a [u8] {}

impl<T, Scope, Aggr, D> SqlCompatible<T, Expr<T, Scope, Aggr, D>> for Expr<T, Scope, Aggr, D> {}

// A single-column query projects exactly one value, so it is comparable with a
// column of that value's type. Queries projecting several columns have no
// `(T,)` output and therefore no such implementation, which is what rejects
// them as operands at the call site.
impl<D, Tables, T> SqlCompatible<T, SelectQuery<D, Tables, (T,), HasProjection>>
    for SelectQuery<D, Tables, (T,), HasProjection>
where
    D: Dialect,
    Tables: TableScope,
{
}

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
