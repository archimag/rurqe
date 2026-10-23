//! Shared parameter binding and row decoding for the PostgreSQL driver adapters.
//!
//! Both adapters hand their statements to the same binding layer: a [`SqlVal`]
//! parameter is encoded into the binary representation the server expects for
//! the column it targets, and a result column is decoded back into the ordered
//! [`SqlVal`]s RURQE rows are built from.

mod array;
mod driver;
mod numeric;
mod network;
mod wire;

#[cfg(test)]
mod tests;

use std::error::Error as StdError;
use std::fmt;

use bytes::BytesMut;
use rurqe_core::expr::SqlVal;

use self::driver::types::{FromSql, IsNull, Kind, ToSql, Type};
use self::driver::{Error as DriverError, Row};
use self::wire::{base_type, wire_of, Wire};

/// Error raised while executing a query through a PostgreSQL driver adapter.
#[derive(Debug)]
pub enum PgError {
    /// The PostgreSQL driver failed to execute the statement.
    Driver(DriverError),
    /// A result column holds a type that cannot be decoded into a [`SqlVal`].
    UnsupportedType(String),
    /// A result column holds a value that does not decode as its declared type.
    InvalidValue {
        /// Name of the column type the value was read as.
        column_type: String,
        /// Why the value does not decode.
        reason: String,
    },
    /// The connection could not be checked out of a `deadpool-postgres` pool.
    #[cfg(feature = "deadpool-postgres")]
    Pool(deadpool_postgres::PoolError),
}

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PgError::Driver(error) => write!(f, "PostgreSQL driver error: {error}"),
            PgError::UnsupportedType(name) => write!(
                f,
                "unsupported PostgreSQL column type `{name}`: cannot decode into a SQL value"
            ),
            PgError::InvalidValue {
                column_type,
                reason,
            } => write!(
                f,
                "invalid value for PostgreSQL column type `{column_type}`: {reason}"
            ),
            #[cfg(feature = "deadpool-postgres")]
            PgError::Pool(error) => write!(f, "connection pool error: {error}"),
        }
    }
}

impl StdError for PgError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            PgError::Driver(error) => Some(error),
            PgError::UnsupportedType(_) => None,
            PgError::InvalidValue { .. } => None,
            #[cfg(feature = "deadpool-postgres")]
            PgError::Pool(_) => None,
        }
    }
}

impl From<DriverError> for PgError {
    fn from(error: DriverError) -> Self {
        PgError::Driver(error)
    }
}

#[cfg(feature = "deadpool-postgres")]
impl From<deadpool_postgres::PoolError> for PgError {
    fn from(error: deadpool_postgres::PoolError) -> Self {
        PgError::Pool(error)
    }
}

/// Binds a text parameter to the binary representation expected by `ty`.
///
/// `UUID`, `JSON`/`JSONB`, and the temporal types have no dedicated
/// [`SqlVal`] variant: codegen maps them to `String`, so their text values
/// are parsed back into the type the column declares before being encoded. A
/// build without the feature backing one of those families binds the text as it
/// stands and leaves the server to reject a value it cannot read.
fn text_to_sql(
    value: &str,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    #[cfg(feature = "uuid")]
    {
        if ty == &Type::UUID {
            return value.parse::<uuid::Uuid>()?.to_sql(ty, out);
        }
    }

    #[cfg(feature = "serde_json")]
    {
        if ty == &Type::JSON || ty == &Type::JSONB {
            let parsed: serde_json::Value = serde_json::from_str(value)?;
            return parsed.to_sql(ty, out);
        }
    }

    #[cfg(feature = "chrono")]
    {
        if ty == &Type::TIMESTAMP {
            return parse_timestamp(value)?.to_sql(ty, out);
        }
        if ty == &Type::TIMESTAMPTZ {
            return parse_timestamptz(value)?.to_sql(ty, out);
        }
        if ty == &Type::DATE {
            return chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")?.to_sql(ty, out);
        }
    }

    value.to_sql(ty, out)
}

/// Parses a timestamp string, accepting both space- and `T`-separated forms.
#[cfg(feature = "chrono")]
fn parse_timestamp(value: &str) -> Result<chrono::NaiveDateTime, chrono::ParseError> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f"))
}

/// Parses a timestamp with time zone, assuming UTC when no offset is present.
#[cfg(feature = "chrono")]
fn parse_timestamptz(value: &str) -> Result<chrono::DateTime<chrono::Utc>, chrono::ParseError> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&chrono::Utc));
    }
    let naive = parse_timestamp(value)?;
    Ok(chrono::DateTime::from_naive_utc_and_offset(
        naive,
        chrono::Utc,
    ))
}

/// A [`SqlVal`] bound to the parameter type expected by the PostgreSQL driver.
#[derive(Debug)]
pub(crate) struct SqlValParam<'a>(pub(crate) &'a SqlVal);

impl ToSql for SqlValParam<'_> {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        // An array column binds one value per element, so the value is encoded by
        // the array codec of its element type.
        if matches!(base_type(ty).kind(), Kind::Array(_)) {
            return encode_array(self.0, ty, out);
        }

        match wire_of(ty) {
            // Textual columns and plain scalars keep the per-value encoding.
            None | Some(Wire::Text) => {}
            // A column the code generator maps onto `String` has no `SqlVal`
            // variant of its own, so the textual value is encoded into the binary
            // representation the server expects for the column type.
            Some(wire) => return encode_wire(wire, self.0, ty, out),
        }

        match self.0 {
            SqlVal::Null => Ok(IsNull::Yes),
            SqlVal::Bool(value) => value.to_sql(ty, out),
            SqlVal::I32(value) => value.to_sql(ty, out),
            SqlVal::I64(value) => value.to_sql(ty, out),
            SqlVal::F64(value) => value.to_sql(ty, out),
            SqlVal::Text(value) => text_to_sql(value, ty, out),
            SqlVal::Bytes(value) => value.as_slice().to_sql(ty, out),
            #[cfg(feature = "uuid")]
            SqlVal::Uuid(value) => {
                if ty == &Type::UUID {
                    value.to_sql(ty, out)
                } else {
                    value.to_string().to_sql(ty, out)
                }
            }
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDateTime(value) => {
                if ty == &Type::TIMESTAMP {
                    value.to_sql(ty, out)
                } else if ty == &Type::TIMESTAMPTZ {
                    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                        *value,
                        chrono::Utc,
                    )
                    .to_sql(ty, out)
                } else {
                    value.to_string().to_sql(ty, out)
                }
            }
            #[cfg(feature = "chrono")]
            SqlVal::DateTimeUtc(value) => {
                if ty == &Type::TIMESTAMPTZ {
                    value.to_sql(ty, out)
                } else if ty == &Type::TIMESTAMP {
                    value.naive_utc().to_sql(ty, out)
                } else {
                    value.to_rfc3339().to_sql(ty, out)
                }
            }
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDate(value) => {
                if ty == &Type::DATE {
                    value.to_sql(ty, out)
                } else {
                    value.to_string().to_sql(ty, out)
                }
            }
            #[cfg(feature = "serde_json")]
            SqlVal::Json(value) => {
                if ty == &Type::JSON || ty == &Type::JSONB {
                    value.to_sql(ty, out)
                } else {
                    value.to_string().to_sql(ty, out)
                }
            }
            SqlVal::EncodingError(message) => {
                Err(Box::new(::std::io::Error::other(message.clone())))
            }
            // `SqlVal` is `#[non_exhaustive]` because several of its variants are
            // gated behind cargo features, so this adapter may observe a value
            // kind it does not know how to encode. Refusing it keeps the failure
            // visible instead of binding a misleading textual form.
            other => Err(unusable_value(other, ty)),
        }
    }

    /// Reports whether a column of type `ty` can receive a bound parameter.
    ///
    /// The list covers every type the adapter encodes natively or through its
    /// textual representation, including the types that carry no dedicated
    /// [`SqlVal`] variant and reach the server as text. Columns outside the list
    /// are rejected before the driver is asked to encode the parameter, which
    /// turns an opaque driver error into an actionable diagnostic.
    fn accepts(ty: &Type) -> bool {
        let ty = base_type(ty);

        // The element type decides whether an array column can receive the
        // parameter, since the column binds one value per element.
        if let Kind::Array(elem_ty) = ty.kind() {
            return Self::accepts(elem_ty);
        }

        matches!(
            ty,
            &Type::BOOL
                | &Type::INT2
                | &Type::INT4
                | &Type::INT8
                | &Type::FLOAT4
                | &Type::FLOAT8
                | &Type::TEXT
                | &Type::VARCHAR
                | &Type::BPCHAR
                | &Type::NAME
                | &Type::UNKNOWN
                | &Type::BYTEA
                | &Type::UUID
                | &Type::JSON
                | &Type::JSONB
                | &Type::TIMESTAMP
                | &Type::TIMESTAMPTZ
                | &Type::DATE
        ) || wire_of(ty).is_some()
    }

    /// Encodes the parameter, rejecting columns of a type the adapter cannot bind.
    ///
    /// `NULL` is bindable for any column type, so an optional parameter keeps
    /// working regardless of the column it is bound to.
    fn to_sql_checked(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        if matches!(self.0, SqlVal::Null) || Self::accepts(ty) {
            return self.to_sql(ty, out);
        }

        Err(unusable_value(self.0, ty))
    }
}

/// Encodes an array parameter into the binary representation of its column.
///
/// Every element is encoded by the codec of the element type, and the values are
/// assembled by the array writer of the driver, which keeps a `NULL` element
/// marked as such.
fn encode_array(
    value: &SqlVal,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    let array_ty = base_type(ty);
    if !matches!(array_ty.kind(), Kind::Array(_)) {
        return Err(unusable_value(value, ty));
    }

    match value {
        SqlVal::Null => Ok(IsNull::Yes),
        SqlVal::Array(elements) => {
            let params: Vec<SqlValParam<'_>> = elements.iter().map(SqlValParam).collect();
            params.to_sql(array_ty, out)
        }
        other => Err(unusable_value(other, ty)),
    }
}

/// Encodes `value` into the binary representation of a textual column type.
///
/// The code generator maps a column without a dedicated [`SqlVal`] variant onto
/// `String`, so such a column receives the textual form of the value and the
/// adapter turns it into the layout the server expects.
fn encode_wire(
    wire: Wire,
    value: &SqlVal,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    if matches!(value, SqlVal::Null) {
        return Ok(IsNull::Yes);
    }

    match (wire, value) {
        (Wire::Numeric, value) => {
            let text = decimal_text(value).ok_or_else(|| unusable_value(value, ty))?;
            numeric::encode(&text, out).map_err(|reason| encoding_failure(&reason))?;
        }
        (Wire::Network { cidr }, SqlVal::Text(text)) => {
            network::encode_address(text, cidr, out).map_err(|reason| encoding_failure(&reason))?;
        }
        (Wire::MacAddr { octets }, SqlVal::Text(text)) => {
            network::encode_hardware(text, octets, out)
                .map_err(|reason| encoding_failure(&reason))?;
        }
        (_, value) => return Err(unusable_value(value, ty)),
    }

    Ok(IsNull::No)
}

/// Renders the decimal form of a value bound to a `NUMERIC` column.
fn decimal_text(value: &SqlVal) -> Option<String> {
    match value {
        SqlVal::Text(text) => Some(text.clone()),
        SqlVal::I32(number) => Some(number.to_string()),
        SqlVal::I64(number) => Some(number.to_string()),
        SqlVal::F64(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Describes a value that cannot be bound to a column of type `ty`.
fn unusable_value(value: &SqlVal, ty: &Type) -> Box<dyn StdError + Sync + Send> {
    Box::new(::std::io::Error::other(format!(
        "cannot bind a SQL `{}` value to a PostgreSQL column of type `{}`",
        value.variant_name(),
        ty.name()
    )))
}

/// Boxes a codec failure into the error type the driver expects.
fn encoding_failure(reason: &str) -> Box<dyn StdError + Sync + Send> {
    Box::new(::std::io::Error::other(reason.to_owned()))
}

/// Raw binary payload of a result column.
///
/// The driver decodes a column only into a type that declares itself compatible
/// with the column type, which leaves the values RURQE reads through its own
/// codecs — arrays, decimals, network addresses, enum labels, extension types,
/// and domains over any of them — out of reach. Reading the payload as raw bytes
/// lets the adapter decode it itself.
struct RawValue(Vec<u8>);

impl<'a> FromSql<'a> for RawValue {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn StdError + Sync + Send>> {
        Ok(RawValue(raw.to_vec()))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// Decodes a driver row into the ordered values RURQE rows are built from.
pub(crate) fn row_to_sql_vals(row: &Row) -> Result<Vec<SqlVal>, PgError> {
    let mut values = Vec::with_capacity(row.columns().len());
    for (index, column) in row.columns().iter().enumerate() {
        values.push(column_value(row, index, column.type_())?);
    }
    Ok(values)
}

/// Why the binary payload of a column cannot be decoded into a [`SqlVal`].
#[derive(Debug)]
enum DecodeError {
    /// The declared column type has no known binary representation.
    Unsupported,
    /// The payload does not decode as its declared type.
    Invalid(String),
}

impl DecodeError {
    /// Describes the failure as one message.
    fn describe(self) -> String {
        match self {
            DecodeError::Unsupported => {
                "the declared type has no known binary representation".to_owned()
            }
            DecodeError::Invalid(reason) => reason,
        }
    }
}

/// Decodes one column of a row into the [`SqlVal`] of its declared type.
fn column_value(row: &Row, index: usize, ty: &Type) -> Result<SqlVal, PgError> {
    match row.try_get::<usize, Option<RawValue>>(index)? {
        None => Ok(SqlVal::Null),
        Some(raw) => decode_value(ty, &raw.0).map_err(|failure| match failure {
            DecodeError::Unsupported => PgError::UnsupportedType(ty.name().to_owned()),
            DecodeError::Invalid(reason) => PgError::InvalidValue {
                column_type: ty.name().to_owned(),
                reason,
            },
        }),
    }
}

/// Decodes the binary payload of one value of the declared type `ty`.
///
/// A column holding an array is decoded element by element, and every other
/// column is decoded directly, so a column and an element of an array of the same
/// type share one codec.
fn decode_value(ty: &Type, payload: &[u8]) -> Result<SqlVal, DecodeError> {
    let ty = base_type(ty);

    if let Kind::Array(elem_ty) = ty.kind() {
        let elements = array::decode(elem_ty, payload).map_err(DecodeError::Invalid)?;
        return Ok(SqlVal::Array(elements));
    }

    scalar_value(ty, payload)
}

/// Decodes the binary payload of a value that is not an array.
fn scalar_value(ty: &Type, payload: &[u8]) -> Result<SqlVal, DecodeError> {
    let value = if ty == &Type::BOOL {
        SqlVal::Bool(bool::from_sql(ty, payload).map_err(decode_failure)?)
    } else if ty == &Type::INT2 {
        SqlVal::I32(i32::from(
            i16::from_sql(ty, payload).map_err(decode_failure)?,
        ))
    } else if ty == &Type::INT4 {
        SqlVal::I32(i32::from_sql(ty, payload).map_err(decode_failure)?)
    } else if ty == &Type::INT8 {
        SqlVal::I64(i64::from_sql(ty, payload).map_err(decode_failure)?)
    } else if ty == &Type::FLOAT4 {
        SqlVal::F64(f64::from(
            f32::from_sql(ty, payload).map_err(decode_failure)?,
        ))
    } else if ty == &Type::FLOAT8 {
        SqlVal::F64(f64::from_sql(ty, payload).map_err(decode_failure)?)
    } else {
        return extended_value(ty, payload);
    };

    Ok(value)
}

/// Decodes a value whose type is recognised through its family rather than
/// through one of the primitive scalars.
fn extended_value(ty: &Type, payload: &[u8]) -> Result<SqlVal, DecodeError> {
    #[cfg(feature = "uuid")]
    {
        if ty == &Type::UUID {
            return Ok(SqlVal::Uuid(
                uuid::Uuid::from_sql(ty, payload).map_err(decode_failure)?,
            ));
        }
    }

    #[cfg(feature = "serde_json")]
    {
        if ty == &Type::JSON || ty == &Type::JSONB {
            return Ok(SqlVal::Json(
                serde_json::Value::from_sql(ty, payload).map_err(decode_failure)?,
            ));
        }
    }

    #[cfg(feature = "chrono")]
    {
        if ty == &Type::TIMESTAMP {
            return Ok(SqlVal::NaiveDateTime(
                chrono::NaiveDateTime::from_sql(ty, payload).map_err(decode_failure)?,
            ));
        }
        if ty == &Type::TIMESTAMPTZ {
            return Ok(SqlVal::DateTimeUtc(
                chrono::DateTime::<chrono::Utc>::from_sql(ty, payload).map_err(decode_failure)?,
            ));
        }
        if ty == &Type::DATE {
            return Ok(SqlVal::NaiveDate(
                chrono::NaiveDate::from_sql(ty, payload).map_err(decode_failure)?,
            ));
        }
    }

    if ty == &Type::TEXT || ty == &Type::VARCHAR || ty == &Type::BPCHAR || ty == &Type::NAME {
        return Ok(SqlVal::Text(
            String::from_sql(ty, payload).map_err(decode_failure)?,
        ));
    }
    if ty == &Type::BYTEA {
        return Ok(SqlVal::Bytes(
            Vec::<u8>::from_sql(ty, payload).map_err(decode_failure)?,
        ));
    }

    // A type outside the native scalars is decoded by the wire codec naming its
    // textual representation, which is also what makes an enum label, an
    // extension type, or a domain over a native type readable.
    match wire_of(ty) {
        Some(wire) => wire
            .decode(payload)
            .map(SqlVal::Text)
            .map_err(DecodeError::Invalid),
        None => Err(DecodeError::Unsupported),
    }
}

/// Turns a driver decoding failure into an invalid-value reason.
fn decode_failure(reason: Box<dyn StdError + Sync + Send>) -> DecodeError {
    DecodeError::Invalid(reason.to_string())
}
