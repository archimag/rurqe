//! Typed row extraction from bound SQL values.
//!
//! A column selection such as `(UserId, UserName)` fixes the row type of a query
//! to `(i64, String)`. [`FromRow`] turns the ordered values of a produced row
//! back into that Rust tuple, so the decoded result stays statically tied to the
//! selection that generated the SQL.

use crate::expr::SqlVal;

/// A Rust value decoded from a single [`SqlVal`].
///
/// The trait is implemented for the scalar types representable as [`SqlVal`]
/// variants: `i32`, `i64`, `f64`, `bool`, and `String`. Optional values wrap any
/// of them and decode the SQL `NULL` value as `None`.
///
/// Engines without dedicated boolean or 32-bit integer storage classes (SQLite
/// reports every `INTEGER` value as [`SqlVal::I64`]) still decode into their
/// declared Rust types: `i32` narrows a 64-bit value that fits, and `bool`
/// accepts the `0` / `1` integer encoding of a boolean column.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be decoded from a SQL value",
    label = "unsupported row value type",
    note = "row values must implement `FromSqlValue`, which is provided for `i32`, `i64`, `f64`, `bool`, `String`, and `Option<T>` for nullable columns"
)]
pub trait FromSqlValue: Sized {
    /// Decodes `value`, returning `None` when it holds a different variant.
    fn from_sql_value(value: &SqlVal) -> Option<Self>;
}

impl FromSqlValue for i32 {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::I32(value) => Some(*value),
            SqlVal::I64(value) => i32::try_from(*value).ok(),
            _ => None,
        }
    }
}

impl FromSqlValue for i64 {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::I64(value) => Some(*value),
            _ => None,
        }
    }
}

impl FromSqlValue for f64 {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::F64(value) => Some(*value),
            _ => None,
        }
    }
}

impl FromSqlValue for bool {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::Bool(value) => Some(*value),
            SqlVal::I32(value) => decode_bool_integer(i64::from(*value)),
            SqlVal::I64(value) => decode_bool_integer(*value),
            _ => None,
        }
    }
}

/// Decodes the `0` / `1` integer encoding engines use for boolean columns.
fn decode_bool_integer(value: i64) -> Option<bool> {
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

impl FromSqlValue for String {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::Text(value) => Some(value.clone()),
            _ => None,
        }
    }
}

impl FromSqlValue for Vec<u8> {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::Bytes(value) => Some(value.clone()),
            _ => None,
        }
    }
}

impl<T: FromSqlValue> FromSqlValue for Option<T> {
    fn from_sql_value(value: &SqlVal) -> Option<Self> {
        match value {
            SqlVal::Null => Some(None),
            _ => T::from_sql_value(value).map(Some),
        }
    }
}

/// A row decoded from the ordered values of a query result.
///
/// The trait is implemented for every tuple of [`FromSqlValue`] elements from
/// arity one up to arity twelve, which is exactly the row type produced by
/// [`Selection`](crate::query::Selection).
///
/// ```
/// use rurqe_core::expr::SqlVal;
/// use rurqe_core::row::FromRow;
///
/// let row = <(i64, String)>::from_row(&[
///     SqlVal::I64(1),
///     SqlVal::Text("root".to_owned()),
/// ]);
///
/// assert_eq!(row, Some((1, "root".to_owned())));
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be decoded from a query row",
    label = "unsupported row type",
    note = "rows are tuples of `FromSqlValue` values with an arity between 1 and 12"
)]
pub trait FromRow: Sized {
    /// Decodes a row from `values`.
    ///
    /// Returns `None` when the number of values differs from the tuple arity or
    /// when a value holds a different SQL type than the slot it decodes into.
    fn from_row(values: &[SqlVal]) -> Option<Self>;
}

macro_rules! impl_from_row {
    ($($value:ident),+ $(,)?) => {
        impl<$($value: FromSqlValue),+> FromRow for ($($value,)+) {
            fn from_row(values: &[SqlVal]) -> Option<Self> {
                let mut values = values.iter();
                let row = ($($value::from_sql_value(values.next()?)?,)+);
                values.next().is_none().then_some(row)
            }
        }
    };
}

impl_from_row!(T1);
impl_from_row!(T1, T2);
impl_from_row!(T1, T2, T3);
impl_from_row!(T1, T2, T3, T4);
impl_from_row!(T1, T2, T3, T4, T5);
impl_from_row!(T1, T2, T3, T4, T5, T6);
impl_from_row!(T1, T2, T3, T4, T5, T6, T7);
impl_from_row!(T1, T2, T3, T4, T5, T6, T7, T8);
impl_from_row!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_from_row!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
impl_from_row!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
impl_from_row!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Selection;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
            UserActive => "active": bool,
        }
    }

    #[test]
    fn decodes_scalar_values() {
        assert_eq!(i32::from_sql_value(&SqlVal::I32(7)), Some(7));
        assert_eq!(i64::from_sql_value(&SqlVal::I64(7)), Some(7));
        assert_eq!(f64::from_sql_value(&SqlVal::F64(0.5)), Some(0.5));
        assert_eq!(bool::from_sql_value(&SqlVal::Bool(true)), Some(true));
        assert_eq!(
            String::from_sql_value(&SqlVal::Text("root".to_owned())),
            Some("root".to_owned())
        );
    }

    #[test]
    fn rejects_values_of_other_variants() {
        assert_eq!(i64::from_sql_value(&SqlVal::Text("7".to_owned())), None);
        assert_eq!(String::from_sql_value(&SqlVal::I64(7)), None);
    }

    #[test]
    fn narrow_integers_accept_wide_integer_values() {
        assert_eq!(i32::from_sql_value(&SqlVal::I64(42)), Some(42));
        assert_eq!(i32::from_sql_value(&SqlVal::I64(i64::MAX)), None);
    }

    #[test]
    fn booleans_accept_integer_encodings() {
        assert_eq!(bool::from_sql_value(&SqlVal::I64(1)), Some(true));
        assert_eq!(bool::from_sql_value(&SqlVal::I32(0)), Some(false));
        assert_eq!(bool::from_sql_value(&SqlVal::I64(2)), None);
    }

    #[test]
    fn decodes_tuples_in_selection_order() {
        let row = <(i64, String, bool)>::from_row(&[
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(true),
        ]);

        assert_eq!(row, Some((1, "root".to_owned(), true)));
    }

    #[test]
    fn single_element_tuple_is_supported() {
        assert_eq!(<(i64,)>::from_row(&[SqlVal::I64(1)]), Some((1,)));
    }

    #[test]
    fn arity_mismatch_is_rejected() {
        assert_eq!(<(i64, String)>::from_row(&[SqlVal::I64(1)]), None);
        assert_eq!(
            <(i64,)>::from_row(&[SqlVal::I64(1), SqlVal::I64(2)]),
            None
        );
    }

    #[test]
    fn value_type_mismatch_is_rejected() {
        assert_eq!(
            <(i64, String)>::from_row(&[
                SqlVal::Text("1".to_owned()),
                SqlVal::Text("root".to_owned()),
            ]),
            None
        );
    }

    #[test]
    fn null_decodes_into_none_for_optional_values() {
        assert_eq!(<Option<i64>>::from_sql_value(&SqlVal::Null), Some(None));
        assert_eq!(
            <Option<String>>::from_sql_value(&SqlVal::Text("root".to_owned())),
            Some(Some("root".to_owned()))
        );
    }

    #[test]
    fn optional_values_reject_foreign_variants() {
        assert_eq!(
            <Option<i64>>::from_sql_value(&SqlVal::Text("7".to_owned())),
            None
        );
    }

    #[test]
    fn selection_rows_are_decodable() {
        fn assert_from_row<R: FromRow>() {}

        assert_from_row::<<(UserId, UserName, UserActive) as Selection>::Row>();
    }
}
