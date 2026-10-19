//! Typed row extraction from bound SQL values.
//!
//! A column selection such as `(UserId, UserName)` fixes the row type of a query
//! to `(i64, String)`. [`FromRow`] turns the ordered values of a produced row
//! back into that Rust tuple, so the decoded result stays statically tied to the
//! selection that generated the SQL.
//!
//! Decoding is the inverse of the parameter encoding performed by
//! [`ToSqlVal`](crate::expr::ToSqlVal): every value type implementing
//! [`FromSqlValue`] also implements `ToSqlVal`, so a value round-trips through a
//! query parameter and a result row. Decoding never panics: a value of another
//! kind, an invalid representation, or a row of unexpected length is reported as
//! a [`RowDecodeError`].

use std::fmt;

use crate::expr::SqlVal;

/// The reason a bound SQL value or a produced row cannot be decoded into Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowDecodeError {
    /// The value holds a kind the target type does not accept.
    TypeMismatch {
        /// The Rust type the value was decoded into.
        target: &'static str,
        /// The kind of the SQL value found in the row.
        found: &'static str,
    },
    /// The value holds the expected kind, but its content is invalid.
    InvalidValue {
        /// The Rust type the value was decoded into.
        target: &'static str,
        /// The kind of the SQL value found in the row.
        found: &'static str,
        /// Why the content cannot be represented by the target type.
        detail: String,
    },
    /// The row holds a number of values the row type does not expect.
    ArityMismatch {
        /// The number of values the row type expects.
        expected: usize,
        /// The number of values the query produced.
        actual: usize,
    },
}

impl fmt::Display for RowDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowDecodeError::TypeMismatch { target, found } => {
                write!(f, "cannot decode a SQL `{found}` value into `{target}`")
            }
            RowDecodeError::InvalidValue {
                target,
                found,
                detail,
            } => write!(
                f,
                "cannot decode a SQL `{found}` value into `{target}`: {detail}"
            ),
            RowDecodeError::ArityMismatch { expected, actual } => write!(
                f,
                "cannot decode a row of {actual} values into a row type of {expected} values"
            ),
        }
    }
}

impl std::error::Error for RowDecodeError {}

/// Builds a [`RowDecodeError::TypeMismatch`] for the target type `T`.
fn type_mismatch<T>(value: &SqlVal) -> RowDecodeError {
    RowDecodeError::TypeMismatch {
        target: std::any::type_name::<T>(),
        found: value.variant_name(),
    }
}

/// Builds a [`RowDecodeError::InvalidValue`] for the target type `T`.
fn invalid_value<T>(value: &SqlVal, detail: &str) -> RowDecodeError {
    RowDecodeError::InvalidValue {
        target: std::any::type_name::<T>(),
        found: value.variant_name(),
        detail: detail.to_owned(),
    }
}

/// A Rust value decoded from a single [`SqlVal`].
///
/// The trait is implemented for the scalar types representable as [`SqlVal`]
/// variants — `i16`, `i32`, `u32`, `i64`, `usize`, `f64`, `bool`, `String`, and
/// `Vec<u8>` — while optional values wrap any of them and decode the SQL `NULL`
/// value as `None`. Every implementation is paired with a [`ToSqlVal`] one, so
/// a value survives the round trip through a parameter and a result row.
///
/// Engines without dedicated boolean, narrow integer, or 32-bit storage classes
/// (SQLite reports every `INTEGER` value as [`SqlVal::I64`]) still decode into
/// their declared Rust types: narrower integers accept a 64-bit value that fits,
/// and `bool` accepts the `0` / `1` integer encoding of a boolean column.
/// Values outside those ranges are reported as [`RowDecodeError::InvalidValue`]
/// rather than silently truncated.
///
/// [`ToSqlVal`]: crate::expr::ToSqlVal
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be decoded from a SQL value",
    label = "unsupported row value type",
    note = "row values must implement `FromSqlValue`, which is provided for `i16`, `i32`, `u32`, `i64`, `usize`, `f64`, `bool`, `String`, `Vec<u8>`, and `Option<T>` for nullable columns"
)]
pub trait FromSqlValue: Sized {
    /// Decodes `value`, reporting why it cannot be represented by `Self`.
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError>;
}

impl FromSqlValue for i16 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(wide) => i16::try_from(*wide)
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into a 16-bit integer")),
            SqlVal::I64(wide) => i16::try_from(*wide)
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into a 16-bit integer")),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for i32 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(narrow) => Ok(*narrow),
            SqlVal::I64(wide) => i32::try_from(*wide)
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into a 32-bit integer")),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for u32 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(narrow) => u32::try_from(i64::from(*narrow))
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into an unsigned 32-bit integer")),
            SqlVal::I64(wide) => u32::try_from(*wide)
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into an unsigned 32-bit integer")),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for i64 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I64(value) => Ok(*value),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for usize {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(narrow) => usize::try_from(i64::from(*narrow))
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into an unsigned index")),
            SqlVal::I64(wide) => usize::try_from(*wide)
                .map_err(|_| invalid_value::<Self>(value, "value does not fit into an unsigned index")),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for f64 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::F64(value) => Ok(*value),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for bool {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Bool(value) => Ok(*value),
            SqlVal::I32(narrow) => decode_bool_integer(i64::from(*narrow))
                .ok_or_else(|| invalid_value::<Self>(value, "booleans accept only 0 or 1")),
            SqlVal::I64(wide) => decode_bool_integer(*wide)
                .ok_or_else(|| invalid_value::<Self>(value, "booleans accept only 0 or 1")),
            _ => Err(type_mismatch::<Self>(value)),
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
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(value) => Ok(value.clone()),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for Vec<u8> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Bytes(value) => Ok(value.clone()),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl<T: FromSqlValue> FromSqlValue for Option<T> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Null => Ok(None),
            _ => T::from_sql_value(value).map(Some),
        }
    }
}

#[cfg(feature = "serde_json")]
impl<T: for<'de> serde::Deserialize<'de>> FromSqlValue for crate::expr::Json<T> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(text) => serde_json::from_str(text.as_str())
                .map(crate::expr::Json)
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            SqlVal::Bytes(bytes) => serde_json::from_slice(bytes.as_slice())
                .map(crate::expr::Json)
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "uuid")]
impl FromSqlValue for uuid::Uuid {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(text) => uuid::Uuid::parse_str(text.as_str())
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            SqlVal::Bytes(bytes) => uuid::Uuid::from_slice(bytes.as_slice())
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "serde_json")]
impl FromSqlValue for serde_json::Value {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(text) => serde_json::from_str(text.as_str())
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            SqlVal::Bytes(bytes) => serde_json::from_slice(bytes.as_slice())
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "chrono")]
impl FromSqlValue for chrono::NaiveDateTime {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(text) => chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f"))
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S"))
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "chrono")]
impl FromSqlValue for chrono::DateTime<chrono::Utc> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(text) => {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
                    return Ok(dt.with_timezone(&chrono::Utc));
                }
                chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f"))
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S"))
                    .map(|naive| chrono::DateTime::from_naive_utc_and_offset(naive, chrono::Utc))
                    .map_err(|e| invalid_value::<Self>(value, &e.to_string()))
            }
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "chrono")]
impl FromSqlValue for chrono::NaiveDate {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(text) => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

/// A row decoded from the ordered values of a query result.
///
/// The trait is implemented for every tuple of [`FromSqlValue`] elements from
/// arity one up to arity twelve, which is exactly the row type produced by
/// [`Selection`](crate::query::Selection), and [`try_from_row`] reports the
/// precise failure — an unexpected number of values, or a value of another kind —
/// instead of collapsing it into `None`.
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
///
/// [`try_from_row`]: FromRow::try_from_row
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be decoded from a query row",
    label = "unsupported row type",
    note = "rows are tuples of `FromSqlValue` values with an arity between 1 and 12"
)]
pub trait FromRow: Sized {
    /// Decodes a row from `values`, reporting why decoding failed.
    ///
    /// Returns [`RowDecodeError::ArityMismatch`] when the number of values
    /// differs from the tuple arity, and the error of the offending value when a
    /// value does not match the slot it decodes into.
    fn try_from_row(values: &[SqlVal]) -> Result<Self, RowDecodeError>;

    /// Decodes a row from `values`, returning `None` on any decoding failure.
    ///
    /// Use [`try_from_row`](FromRow::try_from_row) where the reason of the
    /// failure matters, for example to log the offending value kind.
    fn from_row(values: &[SqlVal]) -> Option<Self> {
        Self::try_from_row(values).ok()
    }
}

macro_rules! impl_from_row {
    ($($value:ident),+ $(,)?) => {
        impl<$($value: FromSqlValue),+> FromRow for ($($value,)+) {
            fn try_from_row(values: &[SqlVal]) -> Result<Self, RowDecodeError> {
                const ARITY: usize = <[&str]>::len(&[$(stringify!($value)),+]);
                let actual = values.len();
                let mut values = values.iter();
                let row = ($(
                    $value::from_sql_value(values.next().ok_or(
                        RowDecodeError::ArityMismatch {
                            expected: ARITY,
                            actual,
                        },
                    )?)?,
                )+);

                if values.next().is_some() {
                    return Err(RowDecodeError::ArityMismatch {
                        expected: ARITY,
                        actual,
                    });
                }

                Ok(row)
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
    use crate::expr::ToSqlVal;
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
        assert_eq!(i16::from_sql_value(&SqlVal::I32(7)), Ok(7));
        assert_eq!(i32::from_sql_value(&SqlVal::I32(7)), Ok(7));
        assert_eq!(u32::from_sql_value(&SqlVal::I64(7)), Ok(7));
        assert_eq!(i64::from_sql_value(&SqlVal::I64(7)), Ok(7));
        assert_eq!(usize::from_sql_value(&SqlVal::I64(7)), Ok(7));
        assert_eq!(f64::from_sql_value(&SqlVal::F64(0.5)), Ok(0.5));
        assert_eq!(bool::from_sql_value(&SqlVal::Bool(true)), Ok(true));
        assert_eq!(
            String::from_sql_value(&SqlVal::Text("root".to_owned())),
            Ok("root".to_owned())
        );
        assert_eq!(
            Vec::<u8>::from_sql_value(&SqlVal::Bytes(vec![1, 2])),
            Ok(vec![1, 2])
        );
    }

    #[test]
    fn rejects_values_of_other_variants() {
        assert_eq!(
            i64::from_sql_value(&SqlVal::Text("7".to_owned())),
            Err(RowDecodeError::TypeMismatch {
                target: std::any::type_name::<i64>(),
                found: "text",
            })
        );
        assert!(String::from_sql_value(&SqlVal::I64(7)).is_err());
        assert!(bool::from_sql_value(&SqlVal::Text("true".to_owned())).is_err());
        assert!(f64::from_sql_value(&SqlVal::I64(7)).is_err());
    }

    #[test]
    fn narrow_integers_accept_wide_integer_values() {
        assert_eq!(i32::from_sql_value(&SqlVal::I64(42)), Ok(42));
        assert_eq!(i16::from_sql_value(&SqlVal::I64(42)), Ok(42));
        assert_eq!(u32::from_sql_value(&SqlVal::I64(42)), Ok(42));
        assert_eq!(usize::from_sql_value(&SqlVal::I32(42)), Ok(42));
    }

    #[test]
    fn out_of_range_values_report_invalid_representations() {
        let error = i32::from_sql_value(&SqlVal::I64(i64::MAX)).expect_err("overflow is rejected");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue {
                found: "int64",
                ..
            }
        ));
        assert!(error.to_string().contains("32-bit integer"));

        let error = u32::from_sql_value(&SqlVal::I32(-1)).expect_err("negatives are rejected");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue {
                found: "int32",
                ..
            }
        ));

        let error =
            i16::from_sql_value(&SqlVal::I64(i64::from(i32::MAX))).expect_err("overflow is rejected");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue {
                found: "int64",
                ..
            }
        ));
    }

    #[test]
    fn booleans_accept_integer_encodings() {
        assert_eq!(bool::from_sql_value(&SqlVal::I64(1)), Ok(true));
        assert_eq!(bool::from_sql_value(&SqlVal::I32(0)), Ok(false));

        let error = bool::from_sql_value(&SqlVal::I64(2)).expect_err("only 0 and 1 decode");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue {
                found: "int64",
                ..
            }
        ));
        assert!(error.to_string().contains("0 or 1"));
    }

    #[test]
    fn decodes_tuples_in_selection_order() {
        let row = <(i64, String, bool)>::try_from_row(&[
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(true),
        ]);

        assert_eq!(row, Ok((1, "root".to_owned(), true)));
    }

    #[test]
    fn single_element_tuple_is_supported() {
        assert_eq!(<(i64,)>::try_from_row(&[SqlVal::I64(1)]), Ok((1,)));
    }

    #[test]
    fn arity_mismatch_is_reported_with_both_lengths() {
        assert_eq!(
            <(i64, String)>::try_from_row(&[SqlVal::I64(1)]),
            Err(RowDecodeError::ArityMismatch {
                expected: 2,
                actual: 1,
            })
        );
        assert_eq!(
            <(i64,)>::try_from_row(&[SqlVal::I64(1), SqlVal::I64(2)]),
            Err(RowDecodeError::ArityMismatch {
                expected: 1,
                actual: 2,
            })
        );
        assert!(RowDecodeError::ArityMismatch {
            expected: 2,
            actual: 1,
        }
        .to_string()
        .contains("row of 1 values"));
    }

    #[test]
    fn value_type_mismatch_is_rejected() {
        assert_eq!(
            <(i64, String)>::try_from_row(&[
                SqlVal::Text("1".to_owned()),
                SqlVal::Text("root".to_owned()),
            ]),
            Err(RowDecodeError::TypeMismatch {
                target: std::any::type_name::<i64>(),
                found: "text",
            })
        );
    }

    #[test]
    fn null_decodes_into_none_for_optional_values() {
        assert_eq!(<Option<i64>>::from_sql_value(&SqlVal::Null), Ok(None));
        assert_eq!(
            <Option<String>>::from_sql_value(&SqlVal::Text("root".to_owned())),
            Ok(Some("root".to_owned()))
        );
    }

    #[test]
    fn optional_values_reject_foreign_variants() {
        assert!(<Option<i64>>::from_sql_value(&SqlVal::Text("7".to_owned())).is_err());
        assert!(<Option<i64>>::from_sql_value(&SqlVal::Null).is_ok());
    }

    #[test]
    fn required_values_report_null_as_a_type_mismatch() {
        let error = i64::from_sql_value(&SqlVal::Null).expect_err("NULL needs an option");

        assert_eq!(
            error,
            RowDecodeError::TypeMismatch {
                target: std::any::type_name::<i64>(),
                found: "null",
            }
        );
    }

    #[test]
    fn values_round_trip_through_the_symmetric_contracts() {
        fn round_trip<T>(value: T) -> T
        where
            T: ToSqlVal + FromSqlValue,
        {
            let encoded = value.to_sql_val();
            T::from_sql_value(&encoded).expect("an encoded value decodes again")
        }

        assert_eq!(round_trip(7i16), 7);
        assert_eq!(round_trip(7i32), 7);
        assert_eq!(round_trip(7u32), 7);
        assert_eq!(round_trip(7i64), 7);
        assert_eq!(round_trip(7usize), 7);
        assert_eq!(round_trip(0.5f64), 0.5);
        assert!(round_trip(true));
        assert_eq!(round_trip("root".to_owned()), "root");
        assert_eq!(round_trip(vec![1u8, 2]), vec![1, 2]);
        assert_eq!(round_trip(Some(7i64)), Some(7));
        assert_eq!(round_trip(None::<i64>), None);
    }

    #[test]
    fn from_row_wraps_try_from_row() {
        assert_eq!(<(i64,)>::from_row(&[SqlVal::I64(1)]), Some((1,)));
        assert_eq!(<(i64,)>::from_row(&[]), None);
        assert_eq!(
            <(i64,)>::from_row(&[SqlVal::Text("1".to_owned())]),
            None
        );
    }

    #[derive(Debug, PartialEq, Eq)]
    struct MockTxid([u8; 4]);

    impl AsRef<[u8]> for MockTxid {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    impl TryFrom<&[u8]> for MockTxid {
        type Error = &'static str;

        fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
            if slice.len() == 4 {
                let mut arr = [0u8; 4];
                arr.copy_from_slice(slice);
                Ok(MockTxid(arr))
            } else {
                Err("invalid length")
            }
        }
    }

    crate::impl_sql_binary_newtype!(MockTxid, 4);

    #[derive(Debug, PartialEq, Eq)]
    struct MockAddress(String);

    impl std::fmt::Display for MockAddress {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::str::FromStr for MockAddress {
        type Err = &'static str;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            if s.starts_with("addr_") {
                Ok(MockAddress(s.to_owned()))
            } else {
                Err("missing addr_ prefix")
            }
        }
    }

    crate::impl_sql_text_newtype!(MockAddress);

    #[test]
    fn custom_binary_newtype_round_trips_and_validates_length() {
        let txid = MockTxid([1, 2, 3, 4]);
        let val = txid.to_sql_val();
        assert_eq!(val, SqlVal::Bytes(vec![1, 2, 3, 4]));
        assert_eq!(MockTxid::from_sql_value(&val), Ok(MockTxid([1, 2, 3, 4])));

        let wrong_len = SqlVal::Bytes(vec![1, 2, 3]);
        assert!(MockTxid::from_sql_value(&wrong_len).is_err());
    }

    #[test]
    fn custom_text_newtype_round_trips_and_validates_parsing() {
        let addr = MockAddress("addr_12345".to_owned());
        let val = addr.to_sql_val();
        assert_eq!(val, SqlVal::Text("addr_12345".to_owned()));
        assert_eq!(
            MockAddress::from_sql_value(&val),
            Ok(MockAddress("addr_12345".to_owned()))
        );

        let invalid = SqlVal::Text("invalid_address".to_owned());
        assert!(MockAddress::from_sql_value(&invalid).is_err());
    }

    #[test]
    fn selection_rows_are_decodable() {
        fn assert_from_row<R: FromRow>() {}

        assert_from_row::<<(UserId, UserName, UserActive) as Selection>::Row>();
    }
}
