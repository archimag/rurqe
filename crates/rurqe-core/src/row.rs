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
/// `bool` accepts the `0` / `1` integer encoding of a boolean column, a floating
/// point value accepts the integer kinds (a JSON document renders an integral
/// value of a float column as a plain number), and a 64-bit integer accepts a
/// floating point value only when it carries no fraction.
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
            SqlVal::I32(wide) => i16::try_from(*wide).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into a 16-bit integer")
            }),
            SqlVal::I64(wide) => i16::try_from(*wide).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into a 16-bit integer")
            }),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for i32 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(narrow) => Ok(*narrow),
            SqlVal::I64(wide) => i32::try_from(*wide).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into a 32-bit integer")
            }),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for u32 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(narrow) => u32::try_from(i64::from(*narrow)).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into an unsigned 32-bit integer")
            }),
            SqlVal::I64(wide) => u32::try_from(*wide).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into an unsigned 32-bit integer")
            }),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for i64 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I64(value) => Ok(*value),
            // An exact decimal column keeps every digit of its value, and an
            // integer field reads the whole number it carries. A value the
            // 64-bit range cannot hold is reported instead of wrapping around.
            #[cfg(feature = "decimal")]
            SqlVal::Decimal(decimal) => {
                use rust_decimal::prelude::ToPrimitive;

                decimal.to_i64().ok_or_else(|| {
                    invalid_value::<Self>(value, "value does not fit into a 64-bit integer")
                })
            }
            // A nested document reports a fractional number as a floating point
            // value when no exact decimal representation is compiled in, and an
            // integral number of the same document is indistinguishable from the
            // integral value of a floating point column. Only a value the 64-bit
            // range carries without a fraction is accepted, so a rounded,
            // fractional, or out-of-range number reports the failure instead of
            // being silently changed by the conversion.
            SqlVal::F64(number) => integral_float(*number).ok_or_else(|| {
                invalid_value::<Self>(value, "value is not an exact 64-bit integer")
            }),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for usize {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::I32(narrow) => usize::try_from(i64::from(*narrow)).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into an unsigned index")
            }),
            SqlVal::I64(wide) => usize::try_from(*wide).map_err(|_| {
                invalid_value::<Self>(value, "value does not fit into an unsigned index")
            }),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for f64 {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::F64(value) => Ok(*value),
            // Engines without a floating point storage class report a `REAL`
            // value as the integer it fits into, and a JSON document renders an
            // integral value of a floating point column as a plain number: both
            // PostgreSQL `to_jsonb` and the object written by `multiset_object`
            // carry `1200.0` as the integral number `1200`. An integer kind
            // therefore decodes into the floating point value of the number
            // instead of reporting a mismatch, exactly like `bool` accepts the
            // integer encoding of a boolean column.
            SqlVal::I32(number) => Ok(f64::from(*number)),
            SqlVal::I64(number) => Ok(*number as f64),
            // A nested document reports a fractional value as the exact decimal
            // holding its digits, so a floating point field of the same row reads
            // the number the document carries.
            #[cfg(feature = "decimal")]
            SqlVal::Decimal(decimal) => {
                use rust_decimal::prelude::ToPrimitive;

                decimal.to_f64().ok_or_else(|| {
                    invalid_value::<Self>(value, "value does not fit into a 64-bit float")
                })
            }
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

/// Reads a floating point value that carries no fraction as the integer it is.
///
/// A JSON document reports a fractional number as a floating point value when no
/// exact decimal representation is compiled in, and an integral number of the
/// same document is indistinguishable from the integral value of a floating
/// point column. Only a value the 64-bit integer range holds without a fraction
/// is read back, so a rounded, fractional, or out-of-range number stays reported
/// instead of being silently changed by the cast.
fn integral_float(value: f64) -> Option<i64> {
    let integer = value as i64;

    if integer as f64 == value {
        Some(integer)
    } else {
        None
    }
}

impl FromSqlValue for String {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Text(value) => Ok(value.clone()),
            // The code generator maps a column of a dedicated value family onto
            // `String` unless its `domain_types` option asks for the exact type,
            // which keeps a generated schema free of the optional value crates.
            // Such a column decodes from the textual form of the value it reads,
            // so the generated type stays the one the column was mapped onto.
            #[cfg(feature = "uuid")]
            SqlVal::Uuid(_) => Ok(value.to_string()),
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDateTime(_) | SqlVal::DateTimeUtc(_) | SqlVal::NaiveDate(_) => {
                Ok(value.to_string())
            }
            #[cfg(feature = "serde_json")]
            SqlVal::Json(_) => Ok(value.to_string()),
            #[cfg(feature = "decimal")]
            SqlVal::Decimal(_) => Ok(value.to_string()),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

impl FromSqlValue for Vec<u8> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Bytes(value) => Ok(value.clone()),
            // A binary field of a nested row reaches the document as its
            // canonical hexadecimal text, and an engine reporting a binary
            // value as text reports the same form, so the text decodes back
            // into the bytes it carries instead of reporting a mismatch.
            SqlVal::Text(text) => hex_decode(text).ok_or_else(|| {
                invalid_value::<Self>(value, "a binary value is encoded as hexadecimal text")
            }),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

/// Encodes `bytes` as the hexadecimal text a JSON document carries for a
/// binary value.
///
/// The text is lowercase and carries no prefix, so a binary field of a nested
/// row stays short and free of the characters JSON escapes. `hex_decode` reads
/// the same form back, together with the prefixed variant PostgreSQL writes,
/// so a binary field round trips whichever engine produced the document.
#[cfg(feature = "serde_json")]
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Decodes the hexadecimal text of a binary value into the bytes it carries.
///
/// Digits are case-insensitive, and the `\x` prefix of the bytea text
/// PostgreSQL writes into a JSON document is accepted, so the text the engine
/// produced decodes exactly like the canonical text `hex_encode` writes. Text
/// that is not hexadecimal reports `None`, leaving the caller to name the
/// target its diagnostic carries.
pub(crate) fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let digits = text.strip_prefix("\\x").unwrap_or(text).as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks_exact(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        bytes.push(high << 4 | low);
    }
    Some(bytes)
}

/// Decodes one hexadecimal digit into the four bits it carries.
fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
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

#[cfg(feature = "decimal")]
impl FromSqlValue for rust_decimal::Decimal {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        use rust_decimal::Decimal;

        match value {
            SqlVal::Decimal(decimal) => Ok(*decimal),
            // An engine without a native decimal type reports the value as the
            // integer, the real, or the text it stores, so each form is read back
            // into the exact type.
            SqlVal::I32(number) => Ok(Decimal::from(*number)),
            SqlVal::I64(number) => Ok(Decimal::from(*number)),
            SqlVal::F64(number) => Decimal::try_from(*number)
                .map_err(|error| invalid_value::<Self>(value, &error.to_string())),
            SqlVal::Text(text) => text
                .parse::<Decimal>()
                .map_err(|error| invalid_value::<Self>(value, &error.to_string())),
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "serde_json")]
impl<T: for<'de> serde::Deserialize<'de>> FromSqlValue for crate::expr::Json<T> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::Json(val) => serde_json::from_value(val.clone())
                .map(crate::expr::Json)
                .map_err(|e| invalid_value::<Self>(value, &e.to_string())),
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
            SqlVal::Uuid(uuid) => Ok(*uuid),
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
            SqlVal::Json(val) => Ok(val.clone()),
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
            SqlVal::NaiveDateTime(dt) => Ok(*dt),
            SqlVal::DateTimeUtc(dt) => Ok(dt.naive_utc()),
            SqlVal::Text(text) => {
                chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
                    })
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S"))
                    .map_err(|e| invalid_value::<Self>(value, &e.to_string()))
            }
            _ => Err(type_mismatch::<Self>(value)),
        }
    }
}

#[cfg(feature = "chrono")]
impl FromSqlValue for chrono::DateTime<chrono::Utc> {
    fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
        match value {
            SqlVal::DateTimeUtc(dt) => Ok(*dt),
            SqlVal::NaiveDateTime(ndt) => Ok(chrono::DateTime::from_naive_utc_and_offset(
                *ndt,
                chrono::Utc,
            )),
            SqlVal::Text(text) => {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
                    return Ok(dt.with_timezone(&chrono::Utc));
                }
                chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
                    })
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
            SqlVal::NaiveDate(date) => Ok(*date),
            SqlVal::NaiveDateTime(dt) => Ok(dt.date()),
            SqlVal::DateTimeUtc(dt) => Ok(dt.date_naive()),
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
        assert!(f64::from_sql_value(&SqlVal::Text("7".to_owned())).is_err());
    }

    #[cfg(feature = "uuid")]
    #[test]
    fn string_columns_decode_a_uuid_in_its_textual_form() {
        assert_eq!(
            String::from_sql_value(&SqlVal::Uuid(uuid::Uuid::from_u128(1))),
            Ok("00000000-0000-0000-0000-000000000001".to_owned())
        );
    }

    #[cfg(feature = "chrono")]
    #[test]
    fn string_columns_decode_temporal_values_in_their_textual_form() {
        let timestamp = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
            .and_then(|date| date.and_hms_opt(3, 4, 5))
            .expect("the timestamp is in the range of the calendar");

        assert_eq!(
            String::from_sql_value(&SqlVal::NaiveDateTime(timestamp)),
            Ok("2024-01-02 03:04:05".to_owned())
        );
        assert_eq!(
            String::from_sql_value(&SqlVal::NaiveDate(timestamp.date())),
            Ok("2024-01-02".to_owned())
        );
        assert_eq!(
            String::from_sql_value(&SqlVal::DateTimeUtc(
                chrono::DateTime::from_naive_utc_and_offset(timestamp, chrono::Utc),
            )),
            Ok("2024-01-02T03:04:05+00:00".to_owned())
        );
    }

    #[cfg(feature = "serde_json")]
    #[test]
    fn string_columns_decode_a_json_document_in_its_textual_form() {
        let document = serde_json::json!({ "active": true });

        assert_eq!(
            String::from_sql_value(&SqlVal::Json(document.clone())),
            Ok(document.to_string())
        );
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn string_columns_decode_an_exact_decimal_in_its_textual_form() {
        assert_eq!(
            String::from_sql_value(&SqlVal::Decimal(rust_decimal::Decimal::new(1500, 3))),
            Ok("1.500".to_owned())
        );
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
            RowDecodeError::InvalidValue { found: "int64", .. }
        ));
        assert!(error.to_string().contains("32-bit integer"));

        let error = u32::from_sql_value(&SqlVal::I32(-1)).expect_err("negatives are rejected");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { found: "int32", .. }
        ));

        let error = i16::from_sql_value(&SqlVal::I64(i64::from(i32::MAX)))
            .expect_err("overflow is rejected");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { found: "int64", .. }
        ));
    }

    #[test]
    fn booleans_accept_integer_encodings() {
        assert_eq!(bool::from_sql_value(&SqlVal::I64(1)), Ok(true));
        assert_eq!(bool::from_sql_value(&SqlVal::I32(0)), Ok(false));

        let error = bool::from_sql_value(&SqlVal::I64(2)).expect_err("only 0 and 1 decode");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { found: "int64", .. }
        ));
        assert!(error.to_string().contains("0 or 1"));
    }

    #[test]
    fn floating_point_values_accept_the_integral_kinds() {
        assert_eq!(f64::from_sql_value(&SqlVal::I32(7)), Ok(7.0));
        assert_eq!(f64::from_sql_value(&SqlVal::I64(7)), Ok(7.0));
        assert_eq!(f64::from_sql_value(&SqlVal::I64(-1200)), Ok(-1200.0));
        assert_eq!(f64::from_sql_value(&SqlVal::F64(0.5)), Ok(0.5));

        assert!(f64::from_sql_value(&SqlVal::Bool(true)).is_err());
        assert!(f64::from_sql_value(&SqlVal::Bytes(vec![1, 2])).is_err());
        assert!(f64::from_sql_value(&SqlVal::Null).is_err());
    }

    #[test]
    fn integer_values_accept_a_floating_point_value_carrying_no_fraction() {
        assert_eq!(i64::from_sql_value(&SqlVal::F64(7.0)), Ok(7));
        assert_eq!(i64::from_sql_value(&SqlVal::F64(-1200.0)), Ok(-1200));

        let error = i64::from_sql_value(&SqlVal::F64(0.5)).expect_err("a fraction is rejected");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { found: "float64", .. }
        ));

        assert!(i64::from_sql_value(&SqlVal::F64(f64::NAN)).is_err());
        assert!(i64::from_sql_value(&SqlVal::F64(f64::INFINITY)).is_err());
        assert!(i64::from_sql_value(&SqlVal::F64(1e30)).is_err());
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
        assert_eq!(<(i64,)>::from_row(&[SqlVal::Text("1".to_owned())]), None);
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
    fn binary_values_decode_from_their_hexadecimal_text() {
        assert_eq!(
            Vec::<u8>::from_sql_value(&SqlVal::Text("000102feff".to_owned())),
            Ok(vec![0x00, 0x01, 0x02, 0xfe, 0xff])
        );
        assert_eq!(
            Vec::<u8>::from_sql_value(&SqlVal::Text("DEAD".to_owned())),
            Ok(vec![0xde, 0xad]),
            "digits are case-insensitive"
        );
        assert_eq!(
            Vec::<u8>::from_sql_value(&SqlVal::Text(r"\x000102feff".to_owned())),
            Ok(vec![0x00, 0x01, 0x02, 0xfe, 0xff]),
            "the bytea text PostgreSQL reports decodes"
        );
        assert_eq!(
            Vec::<u8>::from_sql_value(&SqlVal::Bytes(vec![1, 2])),
            Ok(vec![1, 2]),
            "a binary value keeps its native kind"
        );
    }

    #[test]
    fn binary_values_report_text_that_is_not_hexadecimal() {
        let error = Vec::<u8>::from_sql_value(&SqlVal::Text("[1, 2]".to_owned()))
            .expect_err("the debug rendering of a vector is not hexadecimal");

        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { found: "text", .. }
        ));
        assert!(error.to_string().contains("hexadecimal"));
        assert!(Vec::<u8>::from_sql_value(&SqlVal::Text("010".to_owned())).is_err());
        assert!(Vec::<u8>::from_sql_value(&SqlVal::Text("zz".to_owned())).is_err());
        assert!(Vec::<u8>::from_sql_value(&SqlVal::Text("hello".to_owned())).is_err());
        assert!(Vec::<u8>::from_sql_value(&SqlVal::Bool(true)).is_err());
    }

    #[test]
    fn hexadecimal_text_decodes_into_its_bytes() {
        assert_eq!(hex_decode("0001feff"), Some(vec![0x00, 0x01, 0xfe, 0xff]));
        assert_eq!(hex_decode("0001FEFF"), Some(vec![0x00, 0x01, 0xfe, 0xff]));
        assert_eq!(
            hex_decode(r"\x0001feff"),
            Some(vec![0x00, 0x01, 0xfe, 0xff])
        );
        assert_eq!(hex_decode(r"\x"), Some(Vec::new()));
        assert_eq!(hex_decode("010"), None);
        assert_eq!(hex_decode("zz"), None);
        assert_eq!(hex_decode("hello"), None);
    }

    #[test]
    fn selection_rows_are_decodable() {
        fn assert_from_row<R: FromRow>() {}

        assert_from_row::<<(UserId, UserName, UserActive) as Selection>::Row>();
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn floating_point_values_accept_an_exact_decimal() {
        assert_eq!(
            f64::from_sql_value(&SqlVal::Decimal(rust_decimal::Decimal::new(125, 1))),
            Ok(12.5)
        );
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn integer_values_accept_an_exact_decimal() {
        use rust_decimal::Decimal;

        assert_eq!(
            i64::from_sql_value(&SqlVal::Decimal(Decimal::from(7i64))),
            Ok(7)
        );

        let error = i64::from_sql_value(&SqlVal::Decimal(Decimal::MAX))
            .expect_err("the value exceeds the 64-bit integer range");
        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { found: "decimal", .. }
        ));
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn decimal_values_decode_exactly() {
        use rust_decimal::Decimal;

        let exact = Decimal::new(1500, 3);

        assert_eq!(Decimal::from_sql_value(&SqlVal::Decimal(exact)), Ok(exact));
        assert_eq!(
            Decimal::from_sql_value(&SqlVal::I64(7)),
            Ok(Decimal::from(7i64))
        );
        assert_eq!(
            Decimal::from_sql_value(&SqlVal::Text("1.500".to_owned())),
            Ok(exact)
        );
        assert_eq!(
            Decimal::from_sql_value(&SqlVal::F64(1.5)),
            Ok(Decimal::new(15, 1))
        );
        assert!(Decimal::from_sql_value(&SqlVal::Bool(true)).is_err());
    }
}
