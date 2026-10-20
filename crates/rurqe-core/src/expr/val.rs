//! Bound SQL parameter values, conversion contracts, and the primitive operator enums of expressions.

use core::fmt;

/// A SQL literal bound to a single query parameter.
///
/// Values stay in their Rust representation until the dialect renders them, so
/// the produced parameter list remains typed and allocation-free for scalars.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlVal {
    /// The SQL `NULL` value.
    Null,
    /// A 32-bit signed integer.
    I32(i32),
    /// A 64-bit signed integer.
    I64(i64),
    /// A 64-bit floating point number.
    F64(f64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Text(String),
    /// A binary blob.
    Bytes(Vec<u8>),
    /// A UUID value.
    #[cfg(feature = "uuid")]
    Uuid(uuid::Uuid),
    /// A naive date and time without timezone offset.
    #[cfg(feature = "chrono")]
    NaiveDateTime(chrono::NaiveDateTime),
    /// A UTC date and time.
    #[cfg(feature = "chrono")]
    DateTimeUtc(chrono::DateTime<chrono::Utc>),
    /// A naive date.
    #[cfg(feature = "chrono")]
    NaiveDate(chrono::NaiveDate),
    /// A JSON value.
    #[cfg(feature = "serde_json")]
    Json(serde_json::Value),
    /// A value that could not be encoded into a SQL parameter.
    ///
    /// The variant carries the reason the conversion failed instead of binding
    /// `NULL`, so an unencodable value is reported by the driver rather than
    /// silently changing the meaning of the statement.
    EncodingError(String),
}

impl SqlVal {
    /// The name of the value kind, as reported by decode diagnostics.
    pub fn variant_name(&self) -> &'static str {
        match self {
            SqlVal::Null => "null",
            SqlVal::I32(_) => "int32",
            SqlVal::I64(_) => "int64",
            SqlVal::F64(_) => "float64",
            SqlVal::Bool(_) => "boolean",
            SqlVal::Text(_) => "text",
            SqlVal::Bytes(_) => "bytes",
            #[cfg(feature = "uuid")]
            SqlVal::Uuid(_) => "uuid",
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDateTime(_) => "timestamp",
            #[cfg(feature = "chrono")]
            SqlVal::DateTimeUtc(_) => "timestamptz",
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDate(_) => "date",
            #[cfg(feature = "serde_json")]
            SqlVal::Json(_) => "json",
            SqlVal::EncodingError(_) => "encoding error",
        }
    }
}

impl fmt::Display for SqlVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlVal::Null => f.write_str("NULL"),
            SqlVal::I32(value) => write!(f, "{value}"),
            SqlVal::I64(value) => write!(f, "{value}"),
            SqlVal::F64(value) => write!(f, "{value}"),
            SqlVal::Bool(value) => write!(f, "{value}"),
            SqlVal::Text(value) => f.write_str(value),
            SqlVal::Bytes(value) => write!(f, "{:?}", value),
            #[cfg(feature = "uuid")]
            SqlVal::Uuid(value) => write!(f, "{value}"),
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDateTime(value) => write!(f, "{}", value.format("%Y-%m-%d %H:%M:%S%.f")),
            #[cfg(feature = "chrono")]
            SqlVal::DateTimeUtc(value) => write!(f, "{}", value.to_rfc3339()),
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDate(value) => write!(f, "{}", value.format("%Y-%m-%d")),
            #[cfg(feature = "serde_json")]
            SqlVal::Json(value) => write!(f, "{value}"),
            SqlVal::EncodingError(message) => f.write_str(message),
        }
    }
}

/// Converts a Rust value into the SQL parameter representation bound by queries.
///
/// The trait is the encoding half of the symmetric conversion contract:
/// [`FromSqlValue`](crate::row::FromSqlValue) decodes a [`SqlVal`] back into the
/// same Rust type, so a value round-trips through a query parameter and a result
/// row without an intermediate textual form. Expression builders such as
/// [`IntoExpr`](crate::expr::IntoExpr) encode their operands through this trait,
/// which keeps parameter encoding in a single place.
///
/// Implementations are provided for `i16`, `i32`, `u32`, `i64`, `usize`, `f64`,
/// `bool`, `String`, `&str`, `&String`, `Vec<u8>`, `&[u8]`, [`SqlVal`] itself,
/// and `Option<T>` for every `T: ToSqlVal`, which encodes `None` as
/// [`SqlVal::Null`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be bound as a SQL parameter",
    label = "unsupported parameter type",
    note = "parameters must implement `ToSqlVal`, which is provided for `i16`, `i32`, `u32`, `i64`, `usize`, `f64`, `bool`, `String`, `&str`, `Vec<u8>`, `&[u8]`, and `Option<T>`"
)]
pub trait ToSqlVal: Sized {
    /// Encodes `self` into a bound SQL parameter value.
    fn to_sql_val(self) -> SqlVal;
}

impl ToSqlVal for SqlVal {
    fn to_sql_val(self) -> SqlVal {
        self
    }
}

impl ToSqlVal for i16 {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::I32(i32::from(self))
    }
}

impl ToSqlVal for i32 {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::I32(self)
    }
}

impl ToSqlVal for u32 {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::I64(i64::from(self))
    }
}

impl ToSqlVal for i64 {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::I64(self)
    }
}

impl ToSqlVal for usize {
    fn to_sql_val(self) -> SqlVal {
        match i64::try_from(self) {
            Ok(value) => SqlVal::I64(value),
            Err(_) => SqlVal::EncodingError(format!(
                "the value {self} cannot be represented as a 64-bit signed integer"
            )),
        }
    }
}

impl ToSqlVal for f64 {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::F64(self)
    }
}

impl ToSqlVal for bool {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Bool(self)
    }
}

impl ToSqlVal for String {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Text(self)
    }
}

impl ToSqlVal for &str {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Text(self.to_owned())
    }
}

impl ToSqlVal for &String {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Text(self.clone())
    }
}

impl ToSqlVal for Vec<u8> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Bytes(self)
    }
}

impl ToSqlVal for &[u8] {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Bytes(self.to_vec())
    }
}

impl<T: ToSqlVal> ToSqlVal for Option<T> {
    fn to_sql_val(self) -> SqlVal {
        match self {
            Some(value) => value.to_sql_val(),
            None => SqlVal::Null,
        }
    }
}

/// A transparent wrapper for JSON and JSONB columns.
#[cfg(feature = "serde_json")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Json<T>(pub T);

#[cfg(feature = "serde_json")]
impl<T> core::ops::Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(feature = "serde_json")]
impl<T> core::ops::DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(feature = "serde_json")]
impl<T> From<T> for Json<T> {
    fn from(value: T) -> Self {
        Json(value)
    }
}

/// Encodes `value` as a JSON parameter, reporting values `serde` refuses to serialise.
#[cfg(feature = "serde_json")]
fn encode_json<T: serde::Serialize>(value: &T) -> SqlVal {
    match serde_json::to_value(value) {
        Ok(json) => SqlVal::Json(json),
        Err(error) => SqlVal::EncodingError(format!(
            "the value cannot be encoded as JSON: {error}"
        )),
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> ToSqlVal for Json<T> {
    fn to_sql_val(self) -> SqlVal {
        encode_json(&self.0)
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> ToSqlVal for &Json<T> {
    fn to_sql_val(self) -> SqlVal {
        encode_json(&self.0)
    }
}

#[cfg(feature = "uuid")]
impl ToSqlVal for uuid::Uuid {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Uuid(self)
    }
}

#[cfg(feature = "uuid")]
impl ToSqlVal for &uuid::Uuid {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Uuid(*self)
    }
}

#[cfg(feature = "chrono")]
impl ToSqlVal for chrono::NaiveDateTime {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::NaiveDateTime(self)
    }
}

#[cfg(feature = "chrono")]
impl ToSqlVal for &chrono::NaiveDateTime {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::NaiveDateTime(*self)
    }
}

#[cfg(feature = "chrono")]
impl ToSqlVal for chrono::DateTime<chrono::Utc> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::DateTimeUtc(self)
    }
}

#[cfg(feature = "chrono")]
impl ToSqlVal for &chrono::DateTime<chrono::Utc> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::DateTimeUtc(*self)
    }
}

#[cfg(feature = "chrono")]
impl ToSqlVal for chrono::NaiveDate {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::NaiveDate(self)
    }
}

#[cfg(feature = "chrono")]
impl ToSqlVal for &chrono::NaiveDate {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::NaiveDate(*self)
    }
}

#[cfg(feature = "serde_json")]
impl ToSqlVal for serde_json::Value {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Json(self)
    }
}

#[cfg(feature = "serde_json")]
impl ToSqlVal for &serde_json::Value {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Json(self.clone())
    }
}

impl From<i32> for SqlVal {
    fn from(value: i32) -> Self {
        value.to_sql_val()
    }
}

impl From<i64> for SqlVal {
    fn from(value: i64) -> Self {
        value.to_sql_val()
    }
}

impl From<f64> for SqlVal {
    fn from(value: f64) -> Self {
        value.to_sql_val()
    }
}

impl From<bool> for SqlVal {
    fn from(value: bool) -> Self {
        value.to_sql_val()
    }
}

impl From<String> for SqlVal {
    fn from(value: String) -> Self {
        value.to_sql_val()
    }
}

impl From<&str> for SqlVal {
    fn from(value: &str) -> Self {
        value.to_sql_val()
    }
}

impl From<&String> for SqlVal {
    fn from(value: &String) -> Self {
        value.to_sql_val()
    }
}

impl From<Vec<u8>> for SqlVal {
    fn from(value: Vec<u8>) -> Self {
        value.to_sql_val()
    }
}

impl From<&[u8]> for SqlVal {
    fn from(value: &[u8]) -> Self {
        value.to_sql_val()
    }
}

impl From<u32> for SqlVal {
    fn from(value: u32) -> Self {
        value.to_sql_val()
    }
}

impl From<usize> for SqlVal {
    fn from(value: usize) -> Self {
        value.to_sql_val()
    }
}

impl From<i16> for SqlVal {
    fn from(value: i16) -> Self {
        value.to_sql_val()
    }
}

#[cfg(feature = "uuid")]
impl From<uuid::Uuid> for SqlVal {
    fn from(value: uuid::Uuid) -> Self {
        value.to_sql_val()
    }
}

#[cfg(feature = "chrono")]
impl From<chrono::NaiveDateTime> for SqlVal {
    fn from(value: chrono::NaiveDateTime) -> Self {
        value.to_sql_val()
    }
}

#[cfg(feature = "chrono")]
impl From<chrono::DateTime<chrono::Utc>> for SqlVal {
    fn from(value: chrono::DateTime<chrono::Utc>) -> Self {
        value.to_sql_val()
    }
}

#[cfg(feature = "chrono")]
impl From<chrono::NaiveDate> for SqlVal {
    fn from(value: chrono::NaiveDate) -> Self {
        value.to_sql_val()
    }
}

#[cfg(feature = "serde_json")]
impl From<serde_json::Value> for SqlVal {
    fn from(value: serde_json::Value) -> Self {
        value.to_sql_val()
    }
}

/// Implements [`ToSqlVal`], [`FromSqlValue`](crate::row::FromSqlValue),
/// and expression compatibility for a binary domain newtype.
#[macro_export]
macro_rules! impl_sql_binary_newtype {
    ($ty:ty, $expected_len:expr) => {
        impl $crate::expr::ToSqlVal for $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::expr::SqlVal::Bytes(::std::convert::AsRef::<[u8]>::as_ref(&self).to_vec())
            }
        }

        impl<'a> $crate::expr::ToSqlVal for &'a $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::expr::SqlVal::Bytes(::std::convert::AsRef::<[u8]>::as_ref(self).to_vec())
            }
        }

        impl From<$ty> for $crate::expr::SqlVal {
            fn from(val: $ty) -> Self {
                $crate::expr::ToSqlVal::to_sql_val(val)
            }
        }

        impl $crate::row::FromSqlValue for $ty {
            fn from_sql_value(value: &$crate::expr::SqlVal) -> ::std::result::Result<Self, $crate::row::RowDecodeError> {
                match value {
                    $crate::expr::SqlVal::Bytes(bytes) => {
                        let expected: usize = $expected_len;
                        if bytes.len() != expected {
                            return Err($crate::row::RowDecodeError::InvalidValue {
                                target: ::std::any::type_name::<Self>(),
                                found: "bytes",
                                detail: format!("expected {expected} bytes, got {}", bytes.len()),
                            });
                        }
                        ::std::convert::TryFrom::try_from(bytes.as_slice())
                            .map_err(|e| $crate::row::RowDecodeError::InvalidValue {
                                target: ::std::any::type_name::<Self>(),
                                found: "bytes",
                                detail: format!("{e}"),
                            })
                    }
                    _ => Err($crate::row::RowDecodeError::TypeMismatch {
                        target: ::std::any::type_name::<Self>(),
                        found: value.variant_name(),
                    }),
                }
            }
        }

        impl $crate::expr::IntoExpr<$ty> for $ty {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl<'a> $crate::expr::SqlCompatible<$ty, &'a $ty> for &'a $ty {}

        impl<'a> $crate::expr::IntoExpr<$ty> for &'a $ty {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }
    };
    ($ty:ty) => {
        impl $crate::expr::ToSqlVal for $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::expr::SqlVal::Bytes(::std::convert::AsRef::<[u8]>::as_ref(&self).to_vec())
            }
        }

        impl<'a> $crate::expr::ToSqlVal for &'a $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::expr::SqlVal::Bytes(::std::convert::AsRef::<[u8]>::as_ref(self).to_vec())
            }
        }

        impl From<$ty> for $crate::expr::SqlVal {
            fn from(val: $ty) -> Self {
                $crate::expr::ToSqlVal::to_sql_val(val)
            }
        }

        impl $crate::row::FromSqlValue for $ty {
            fn from_sql_value(value: &$crate::expr::SqlVal) -> ::std::result::Result<Self, $crate::row::RowDecodeError> {
                match value {
                    $crate::expr::SqlVal::Bytes(bytes) => {
                        ::std::convert::TryFrom::try_from(bytes.as_slice())
                            .map_err(|e| $crate::row::RowDecodeError::InvalidValue {
                                target: ::std::any::type_name::<Self>(),
                                found: "bytes",
                                detail: format!("{e}"),
                            })
                    }
                    _ => Err($crate::row::RowDecodeError::TypeMismatch {
                        target: ::std::any::type_name::<Self>(),
                        found: value.variant_name(),
                    }),
                }
            }
        }

        impl $crate::expr::IntoExpr<$ty> for $ty {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl<'a> $crate::expr::SqlCompatible<$ty, &'a $ty> for &'a $ty {}

        impl<'a> $crate::expr::IntoExpr<$ty> for &'a $ty {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }
    };
}

/// Implements [`ToSqlVal`], [`FromSqlValue`](crate::row::FromSqlValue),
/// and expression compatibility for a text domain newtype.
#[macro_export]
macro_rules! impl_sql_text_newtype {
    ($ty:ty) => {
        impl $crate::expr::ToSqlVal for $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::expr::SqlVal::Text(::std::string::ToString::to_string(&self))
            }
        }

        impl<'a> $crate::expr::ToSqlVal for &'a $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::expr::SqlVal::Text(::std::string::ToString::to_string(self))
            }
        }

        impl From<$ty> for $crate::expr::SqlVal {
            fn from(val: $ty) -> Self {
                $crate::expr::ToSqlVal::to_sql_val(val)
            }
        }

        impl $crate::row::FromSqlValue for $ty {
            fn from_sql_value(value: &$crate::expr::SqlVal) -> ::std::result::Result<Self, $crate::row::RowDecodeError> {
                match value {
                    $crate::expr::SqlVal::Text(text) => {
                        <Self as ::std::str::FromStr>::from_str(text.as_str())
                            .map_err(|e| $crate::row::RowDecodeError::InvalidValue {
                                target: ::std::any::type_name::<Self>(),
                                found: "text",
                                detail: format!("{e}"),
                            })
                    }
                    _ => Err($crate::row::RowDecodeError::TypeMismatch {
                        target: ::std::any::type_name::<Self>(),
                        found: value.variant_name(),
                    }),
                }
            }
        }

        impl $crate::expr::IntoExpr<$ty> for $ty {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl<'a> $crate::expr::SqlCompatible<$ty, &'a $ty> for &'a $ty {}

        impl<'a> $crate::expr::IntoExpr<$ty> for &'a $ty {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl<'a> $crate::expr::SqlCompatible<$ty, &'a str> for &'a str {}

        impl<'a> $crate::expr::IntoExpr<$ty> for &'a str {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::SqlVal::Text(self.to_owned()))
            }
        }

        impl $crate::expr::SqlCompatible<$ty, ::std::string::String> for ::std::string::String {}

        impl $crate::expr::IntoExpr<$ty> for ::std::string::String {
            type Scope = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::SqlVal::Text(self))
            }
        }
    };
}

/// An aggregate function applied to a column or expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateFunc {
    /// `COUNT`
    Count,
    /// `SUM`
    Sum,
    /// `AVG`
    Avg,
    /// `MIN`
    Min,
    /// `MAX`
    Max,
}

impl AggregateFunc {
    /// The SQL name of the aggregate function.
    pub const fn as_str(self) -> &'static str {
        match self {
            AggregateFunc::Count => "COUNT",
            AggregateFunc::Sum => "SUM",
            AggregateFunc::Avg => "AVG",
            AggregateFunc::Min => "MIN",
            AggregateFunc::Max => "MAX",
        }
    }
}

/// The binary operator joining two sub-expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `>`
    Gt,
    /// `<`
    Lt,
    /// `AND`
    And,
    /// `OR`
    Or,
}

impl BinaryOp {
    /// The SQL text of the operator.
    pub const fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Gt => ">",
            BinaryOp::Lt => "<",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_kinds_report_stable_names() {
        assert_eq!(SqlVal::Null.variant_name(), "null");
        assert_eq!(SqlVal::I32(1).variant_name(), "int32");
        assert_eq!(SqlVal::I64(1).variant_name(), "int64");
        assert_eq!(SqlVal::F64(1.0).variant_name(), "float64");
        assert_eq!(SqlVal::Bool(true).variant_name(), "boolean");
        assert_eq!(SqlVal::Text(String::new()).variant_name(), "text");
        assert_eq!(SqlVal::Bytes(Vec::new()).variant_name(), "bytes");
        #[cfg(feature = "uuid")]
        assert_eq!(SqlVal::Uuid(uuid::Uuid::nil()).variant_name(), "uuid");
        #[cfg(feature = "chrono")]
        {
            assert_eq!(
                SqlVal::NaiveDateTime(chrono::NaiveDateTime::default()).variant_name(),
                "timestamp"
            );
            assert_eq!(
                SqlVal::DateTimeUtc(chrono::DateTime::default()).variant_name(),
                "timestamptz"
            );
            assert_eq!(
                SqlVal::NaiveDate(chrono::NaiveDate::default()).variant_name(),
                "date"
            );
        }
        #[cfg(feature = "serde_json")]
        assert_eq!(SqlVal::Json(serde_json::Value::Null).variant_name(), "json");
    }

    #[test]
    fn to_sql_val_encodes_scalar_values() {
        assert_eq!(7i16.to_sql_val(), SqlVal::I32(7));
        assert_eq!(7i32.to_sql_val(), SqlVal::I32(7));
        assert_eq!(7u32.to_sql_val(), SqlVal::I64(7));
        assert_eq!(7i64.to_sql_val(), SqlVal::I64(7));
        assert_eq!(7usize.to_sql_val(), SqlVal::I64(7));
        assert_eq!(0.5f64.to_sql_val(), SqlVal::F64(0.5));
        assert_eq!(true.to_sql_val(), SqlVal::Bool(true));
        assert_eq!(
            String::from("root").to_sql_val(),
            SqlVal::Text("root".to_owned())
        );
        assert_eq!(
            Vec::from([1u8, 2]).to_sql_val(),
            SqlVal::Bytes(vec![1, 2])
        );
        assert_eq!(SqlVal::I64(1).to_sql_val(), SqlVal::I64(1));
    }

    #[test]
    fn borrowed_values_are_encoded_by_copy() {
        let name = String::from("root");
        let bytes: &[u8] = &[1, 2];

        assert_eq!("root".to_sql_val(), SqlVal::Text("root".to_owned()));
        assert_eq!((&name).to_sql_val(), SqlVal::Text("root".to_owned()));
        assert_eq!(bytes.to_sql_val(), SqlVal::Bytes(vec![1, 2]));
        assert_eq!(name, "root");
    }

    #[test]
    fn optional_values_encode_none_as_null() {
        assert_eq!(Some(7i64).to_sql_val(), SqlVal::I64(7));
        assert_eq!(None::<i64>.to_sql_val(), SqlVal::Null);
        assert_eq!(
            Some(String::from("root")).to_sql_val(),
            SqlVal::Text("root".to_owned())
        );
    }

    #[test]
    fn from_conversions_delegate_to_the_trait() {
        assert_eq!(SqlVal::from(7i64), 7i64.to_sql_val());
        assert_eq!(SqlVal::from(7u32), 7u32.to_sql_val());
        assert_eq!(SqlVal::from("root"), "root".to_sql_val());
        assert_eq!(SqlVal::from(1.5f64), 1.5f64.to_sql_val());
    }

    #[test]
    fn encoding_failures_carry_their_reason() {
        let failure = SqlVal::EncodingError("cannot encode".to_owned());

        assert_eq!(failure.variant_name(), "encoding error");
        assert_eq!(failure.to_string(), "cannot encode");
    }

    #[test]
    fn indices_within_the_signed_range_encode_as_integers() {
        assert_eq!(7usize.to_sql_val(), SqlVal::I64(7));
        assert_eq!(0usize.to_sql_val(), SqlVal::I64(0));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn indices_above_the_signed_range_report_a_failure() {
        let wide = (i64::MAX as usize) + 1;

        let encoded = wide.to_sql_val();
        assert!(matches!(encoded, SqlVal::EncodingError(_)));
        assert!(encoded.to_string().contains("64-bit signed integer"));
    }

    #[cfg(feature = "serde_json")]
    #[test]
    fn unencodable_json_values_report_the_failure() {
        struct Unencodable;

        impl serde::Serialize for Unencodable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("not serialisable"))
            }
        }

        let encoded = Json(Unencodable).to_sql_val();
        assert!(matches!(encoded, SqlVal::EncodingError(_)));
        assert!(encoded.to_string().contains("not serialisable"));

        assert_eq!(
            Json(1i64).to_sql_val(),
            SqlVal::Json(serde_json::json!(1))
        );
    }
}
