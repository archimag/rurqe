//! Bound SQL parameter values, conversion contracts, and the primitive operator enums of expressions.

use core::fmt;

mod newtype;

/// A SQL literal bound to a single query parameter.
///
/// Values stay in their Rust representation until the dialect renders them, so
/// the produced parameter list remains typed and allocation-free for scalars.
///
/// The enumeration is `#[non_exhaustive]` because several of its variants are
/// gated behind cargo features: `Decimal`, `Uuid`, `NaiveDateTime`,
/// `DateTimeUtc`, `NaiveDate`, and `Json` exist only when their feature is
/// enabled. The set of
/// variants a downstream crate observes therefore depends on the enabled
/// features, so matching code outside this crate has to keep a wildcard arm,
/// which stays correct no matter which features are compiled in.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SqlVal {
    /// The SQL `NULL` value.
    Null,
    /// A 32-bit signed integer.
    I32(i32),
    /// A 64-bit signed integer.
    I64(i64),
    /// A 64-bit floating point number.
    F64(f64),
    /// An exact decimal number, carrying the scale it was declared with.
    ///
    /// The variant exists only with the `decimal` cargo feature: a `NUMERIC` or
    /// `DECIMAL` column keeps its exact value instead of passing through a
    /// floating point number that cannot represent every digit of it.
    #[cfg(feature = "decimal")]
    Decimal(rust_decimal::Decimal),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Text(String),
    /// A binary blob.
    Bytes(Vec<u8>),
    /// A SQL array value, carrying one value per element in declaration order.
    ///
    /// Every element keeps the [`SqlVal`] representation of its scalar type, so
    /// the parameter of an array column is described exactly like the parameters
    /// the same values would be bound with one by one.
    Array(Vec<SqlVal>),
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
            #[cfg(feature = "decimal")]
            SqlVal::Decimal(_) => "decimal",
            SqlVal::Bool(_) => "boolean",
            SqlVal::Text(_) => "text",
            SqlVal::Bytes(_) => "bytes",
            SqlVal::Array(_) => "array",
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
            #[cfg(feature = "decimal")]
            SqlVal::Decimal(value) => write!(f, "{value}"),
            SqlVal::Bool(value) => write!(f, "{value}"),
            SqlVal::Text(value) => f.write_str(value),
            SqlVal::Bytes(value) => write!(f, "{:?}", value),
            SqlVal::Array(values) => {
                f.write_str("{")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{value}")?;
                }
                f.write_str("}")
            }
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

#[cfg(feature = "decimal")]
impl ToSqlVal for rust_decimal::Decimal {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Decimal(self)
    }
}

#[cfg(feature = "decimal")]
impl ToSqlVal for &rust_decimal::Decimal {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Decimal(*self)
    }
}

// A decimal column is lifted into its optional form by the same relational rule
// every other scalar follows, so an outer join keeps the exact type of the value.
#[cfg(feature = "decimal")]
impl crate::query::MakeNullable for rust_decimal::Decimal {
    type Nullable = Option<rust_decimal::Decimal>;
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
        Err(error) => {
            SqlVal::EncodingError(format!("the value cannot be encoded as JSON: {error}"))
        }
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

#[cfg(feature = "decimal")]
impl From<rust_decimal::Decimal> for SqlVal {
    fn from(value: rust_decimal::Decimal) -> Self {
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
    /// `>=`
    Gte,
    /// `<=`
    Lte,
    /// `AND`
    And,
    /// `OR`
    Or,
    /// The `+` operator.
    Add,
    /// The `-` operator.
    Sub,
    /// The `*` operator.
    Mul,
    /// The `/` operator.
    Div,
    /// The `%` operator.
    Rem,
}

impl BinaryOp {
    /// The SQL text of the operator.
    pub const fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Gt => ">",
            BinaryOp::Lt => "<",
            BinaryOp::Gte => ">=",
            BinaryOp::Lte => "<=",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
        }
    }
}

#[cfg(test)]
mod tests;
