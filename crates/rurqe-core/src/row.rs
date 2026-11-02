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
///
/// The enumeration is `#[non_exhaustive]`: it names one variant per kind of
/// decoding failure, and every value type reached through a cargo feature, every
/// engine reporting a value in another form, and every nullable layer wrapping the
/// one below it can describe a failure the current set does not name. A new kind
/// therefore arrives as a variant of its own rather than as a change of contract,
/// so matching code outside this crate has to keep a wildcard arm, which stays
/// correct no matter which decoders are compiled in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    // The 64-bit integer range as floating point bounds: `i64::MIN` is exactly
    // representable, while `i64::MAX` is not, so `2^63` is the smallest value
    // the positive side excludes. The bounds are checked before the cast,
    // because `as i64` saturates: `2^63` would land on `i64::MAX` and a value
    // below `i64::MIN` on `i64::MIN`, silently changing a number the integer
    // range does not hold.
    const LOWER_BOUND: f64 = -9_223_372_036_854_775_808.0;
    const UPPER_BOUND: f64 = 9_223_372_036_854_775_808.0;

    let in_range = (LOWER_BOUND..UPPER_BOUND).contains(&value);

    if value.is_finite() && value.fract() == 0.0 && in_range {
        Some(value as i64)
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
mod tests;
