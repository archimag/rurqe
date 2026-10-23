//! Shared parameter binding and row decoding for the PostgreSQL driver adapters.

mod numeric;
mod network;
mod wire;

use std::error::Error as StdError;
use std::fmt;

use bytes::BytesMut;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::BuiltQuery;
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type};
use tokio_postgres::Row;

use wire::{wire_of, Wire};

/// Error raised while executing a query through a PostgreSQL driver adapter.
#[derive(Debug)]
pub enum PgError {
    /// The PostgreSQL driver failed to execute the statement.
    Driver(tokio_postgres::Error),
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

impl From<tokio_postgres::Error> for PgError {
    fn from(error: tokio_postgres::Error) -> Self {
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
/// are parsed back into the type the column declares before being encoded.
fn text_to_sql(
    value: &str,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    if ty == &Type::UUID {
        value.parse::<uuid::Uuid>()?.to_sql(ty, out)
    } else if ty == &Type::JSON || ty == &Type::JSONB {
        let parsed: serde_json::Value = serde_json::from_str(value)?;
        parsed.to_sql(ty, out)
    } else if ty == &Type::TIMESTAMP {
        parse_timestamp(value)?.to_sql(ty, out)
    } else if ty == &Type::TIMESTAMPTZ {
        parse_timestamptz(value)?.to_sql(ty, out)
    } else if ty == &Type::DATE {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")?.to_sql(ty, out)
    } else {
        value.to_sql(ty, out)
    }
}

/// Parses a timestamp string, accepting both space- and `T`-separated forms.
fn parse_timestamp(value: &str) -> Result<chrono::NaiveDateTime, chrono::ParseError> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f"))
}

/// Parses a timestamp with time zone, assuming UTC when no offset is present.
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

/// Executes `query` through an asynchronous client without decoding rows.
///
/// The helper accepts any [`tokio_postgres::GenericClient`] — a plain
/// client, a pooled client, or an open transaction — so the parameter
/// binding logic is shared by every asynchronous adapter.
pub(crate) async fn fetch_raw_async<C>(
    client: &C,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, PgError>
where
    C: tokio_postgres::GenericClient,
{
    let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
    let refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|param| param as &(dyn ToSql + Sync))
        .collect();
    let rows = client.query(query.sql.as_str(), &refs).await?;
    rows.iter().map(row_to_sql_vals).collect()
}

/// Executes `query` through an asynchronous client and returns affected rows.
pub(crate) async fn execute_raw_async<C>(
    client: &C,
    query: &BuiltQuery,
) -> Result<u64, PgError>
where
    C: tokio_postgres::GenericClient,
{
    let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
    let refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|param| param as &(dyn ToSql + Sync))
        .collect();
    Ok(client.execute(query.sql.as_str(), &refs).await?)
}

/// Executes `query` through a synchronous client without decoding rows.
#[cfg(feature = "postgres")]
pub(crate) fn fetch_raw_sync(
    client: &mut postgres::Client,
    query: &BuiltQuery,
) -> Result<Vec<Vec<SqlVal>>, PgError> {
    let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
    let refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|param| param as &(dyn ToSql + Sync))
        .collect();
    let rows = client.query(query.sql.as_str(), &refs)?;
    rows.iter().map(row_to_sql_vals).collect()
}

/// Executes `query` through a synchronous client and returns affected rows.
#[cfg(feature = "postgres")]
pub(crate) fn execute_raw_sync(
    client: &mut postgres::Client,
    query: &BuiltQuery,
) -> Result<u64, PgError> {
    let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
    let refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|param| param as &(dyn ToSql + Sync))
        .collect();
    Ok(client.execute(query.sql.as_str(), &refs)?)
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
/// with the column type, which leaves the types RURQE reads textually — enum
/// labels, extension types, and domains — out of reach. Reading the payload as
/// raw bytes lets the adapter decode it through its own wire codec instead.
struct RawValue(Vec<u8>);

impl RawValue {
    /// Decodes the payload according to the representation `wire` describes.
    fn decode(&self, wire: Wire, ty: &Type) -> Result<String, PgError> {
        wire.decode(&self.0).map_err(|reason| PgError::InvalidValue {
            column_type: ty.name().to_owned(),
            reason,
        })
    }
}

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
        let ty = column.type_();
        let value = if ty == &Type::BOOL {
            row.try_get::<usize, Option<bool>>(index)?
                .map_or(SqlVal::Null, SqlVal::Bool)
        } else if ty == &Type::INT2 {
            row.try_get::<usize, Option<i16>>(index)?
                .map_or(SqlVal::Null, |value| SqlVal::I32(i32::from(value)))
        } else if ty == &Type::INT4 {
            row.try_get::<usize, Option<i32>>(index)?
                .map_or(SqlVal::Null, SqlVal::I32)
        } else if ty == &Type::INT8 {
            row.try_get::<usize, Option<i64>>(index)?
                .map_or(SqlVal::Null, SqlVal::I64)
        } else if ty == &Type::FLOAT4 {
            row.try_get::<usize, Option<f32>>(index)?
                .map_or(SqlVal::Null, |value| SqlVal::F64(f64::from(value)))
        } else if ty == &Type::FLOAT8 {
            row.try_get::<usize, Option<f64>>(index)?
                .map_or(SqlVal::Null, SqlVal::F64)
        } else if ty == &Type::UUID {
            row.try_get::<usize, Option<uuid::Uuid>>(index)?
                .map_or(SqlVal::Null, SqlVal::Uuid)
        } else if ty == &Type::JSON || ty == &Type::JSONB {
            row.try_get::<usize, Option<serde_json::Value>>(index)?
                .map_or(SqlVal::Null, SqlVal::Json)
        } else if ty == &Type::TIMESTAMP {
            row.try_get::<usize, Option<chrono::NaiveDateTime>>(index)?
                .map_or(SqlVal::Null, SqlVal::NaiveDateTime)
        } else if ty == &Type::TIMESTAMPTZ {
            row.try_get::<usize, Option<chrono::DateTime<chrono::Utc>>>(index)?
                .map_or(SqlVal::Null, SqlVal::DateTimeUtc)
        } else if ty == &Type::DATE {
            row.try_get::<usize, Option<chrono::NaiveDate>>(index)?
                .map_or(SqlVal::Null, SqlVal::NaiveDate)
        } else if ty == &Type::TEXT
            || ty == &Type::VARCHAR
            || ty == &Type::BPCHAR
            || ty == &Type::NAME
        {
            row.try_get::<usize, Option<String>>(index)?
                .map_or(SqlVal::Null, SqlVal::Text)
        } else if ty == &Type::BYTEA {
            row.try_get::<usize, Option<Vec<u8>>>(index)?
                .map_or(SqlVal::Null, SqlVal::Bytes)
        } else if let Some(wire) = wire_of(ty) {
            // A column outside the native types is read as the raw payload of its
            // binary representation and decoded by the wire codec, which is also
            // what makes a domain over a native type readable.
            match row.try_get::<usize, Option<RawValue>>(index)? {
                Some(raw) => SqlVal::Text(raw.decode(wire, ty)?),
                None => SqlVal::Null,
            }
        } else {
            return Err(PgError::UnsupportedType(ty.name().to_owned()));
        };
        values.push(value);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use rurqe_core::expr::SqlVal;
    use tokio_postgres::types::{IsNull, Kind, ToSql, Type};

    use super::{wire_of, SqlValParam, Wire};

    /// Builds a type the way the server would report it.
    fn synthetic(name: &str, oid: u32, kind: Kind) -> Type {
        Type::new(name.to_owned(), oid, kind, "public".to_owned())
    }

    #[test]
    fn uuid_json_and_temporal_values_bind_directly() {
        let mut buffer = BytesMut::new();
        let uuid_param = SqlValParam(&SqlVal::Uuid(uuid::Uuid::nil()));
        assert!(matches!(
            uuid_param.to_sql(&Type::UUID, &mut buffer).unwrap(),
            IsNull::No
        ));

        let mut buffer = BytesMut::new();
        let json_param = SqlValParam(&SqlVal::Json(serde_json::json!({ "a": 1 })));
        assert!(matches!(
            json_param.to_sql(&Type::JSONB, &mut buffer).unwrap(),
            IsNull::No
        ));

        let mut buffer = BytesMut::new();
        let timestamp = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap();
        let timestamp_param = SqlValParam(&SqlVal::NaiveDateTime(timestamp));
        assert!(matches!(
            timestamp_param.to_sql(&Type::TIMESTAMP, &mut buffer).unwrap(),
            IsNull::No
        ));

        let mut buffer = BytesMut::new();
        let date = chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let date_param = SqlValParam(&SqlVal::NaiveDate(date));
        assert!(matches!(
            date_param.to_sql(&Type::DATE, &mut buffer).unwrap(),
            IsNull::No
        ));
    }

    #[test]
    fn text_parameters_bind_to_uuid_json_and_temporal_types() {
        let mut buffer = BytesMut::new();
        let uuid_param = SqlValParam(&SqlVal::Text(
            "00000000-0000-0000-0000-000000000000".to_owned(),
        ));
        assert!(matches!(
            uuid_param.to_sql(&Type::UUID, &mut buffer).unwrap(),
            IsNull::No
        ));

        let mut buffer = BytesMut::new();
        let json_param = SqlValParam(&SqlVal::Text("{\"a\":1}".to_owned()));
        assert!(matches!(
            json_param.to_sql(&Type::JSONB, &mut buffer).unwrap(),
            IsNull::No
        ));

        let mut buffer = BytesMut::new();
        let timestamp_param = SqlValParam(&SqlVal::Text("2024-01-02 03:04:05".to_owned()));
        assert!(matches!(
            timestamp_param.to_sql(&Type::TIMESTAMP, &mut buffer).unwrap(),
            IsNull::No
        ));

        let mut buffer = BytesMut::new();
        let date_param = SqlValParam(&SqlVal::Text("2024-01-02".to_owned()));
        assert!(matches!(
            date_param.to_sql(&Type::DATE, &mut buffer).unwrap(),
            IsNull::No
        ));
    }

    #[test]
    fn scalar_values_map_to_driver_parameters() {
        let mut buffer = BytesMut::new();
        assert!(matches!(
            SqlValParam(&SqlVal::I64(7))
                .to_sql(&Type::INT8, &mut buffer)
                .unwrap(),
            IsNull::No
        ));
        assert!(matches!(
            SqlValParam(&SqlVal::Bool(true))
                .to_sql(&Type::BOOL, &mut buffer)
                .unwrap(),
            IsNull::No
        ));
        assert!(matches!(
            SqlValParam(&SqlVal::Text("root".to_owned()))
                .to_sql(&Type::TEXT, &mut buffer)
                .unwrap(),
            IsNull::No
        ));
    }

    #[test]
    fn null_binds_for_any_declared_type() {
        let mut buffer = BytesMut::new();
        assert!(matches!(
            SqlValParam(&SqlVal::Null)
                .to_sql(&Type::INT8, &mut buffer)
                .unwrap(),
            IsNull::Yes
        ));
        assert!(matches!(
            SqlValParam(&SqlVal::Null)
                .to_sql(&Type::UUID, &mut buffer)
                .unwrap(),
            IsNull::Yes
        ));
    }

    #[test]
    fn unusable_column_types_are_rejected_with_a_clear_message() {
        let mut buffer = BytesMut::new();
        let error = match SqlValParam(&SqlVal::I64(1)).to_sql_checked(&Type::INTERVAL, &mut buffer)
        {
            Ok(_) => panic!("intervals cannot receive bound parameters"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("int64"));
        assert!(error.to_string().contains("interval"));
    }

    #[test]
    fn null_binds_to_columns_outside_the_supported_types() {
        let mut buffer = BytesMut::new();

        assert!(matches!(
            SqlValParam(&SqlVal::Null)
                .to_sql_checked(&Type::INTERVAL, &mut buffer)
                .unwrap(),
            IsNull::Yes
        ));
    }

    #[test]
    fn encoding_failures_surface_as_driver_errors() {
        let mut buffer = BytesMut::new();
        let error = match SqlValParam(&SqlVal::EncodingError("not encodable".to_owned()))
            .to_sql_checked(&Type::TEXT, &mut buffer)
        {
            Ok(_) => panic!("an encoding failure cannot be bound"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("not encodable"));
    }

    #[test]
    fn numeric_columns_bind_the_decimal_text_of_the_value() {
        let mut buffer = BytesMut::new();
        let param = SqlValParam(&SqlVal::Text("12345.678".to_owned()));
        assert!(matches!(
            param.to_sql_checked(&Type::NUMERIC, &mut buffer).unwrap(),
            IsNull::No
        ));
        assert_eq!(
            buffer.to_vec(),
            vec![0u8, 3, 0, 1, 0, 0, 0, 3, 0, 1, 9, 0x29, 0x1a, 0x7c]
        );

        // A value bound as a number reaches the same decimal representation.
        let mut buffer = BytesMut::new();
        SqlValParam(&SqlVal::I64(42))
            .to_sql_checked(&Type::NUMERIC, &mut buffer)
            .unwrap();
        assert_eq!(buffer.to_vec(), vec![0u8, 1, 0, 0, 0, 0, 0, 0, 0, 42]);
        assert_eq!(Wire::Numeric.decode(&buffer[..]), Ok("42".to_owned()));
    }

    #[test]
    fn network_columns_bind_and_decode_their_textual_address() {
        let mut buffer = BytesMut::new();
        SqlValParam(&SqlVal::Text("10.1.2.0/24".to_owned()))
            .to_sql_checked(&Type::CIDR, &mut buffer)
            .unwrap();
        assert_eq!(buffer.to_vec(), vec![2u8, 24, 1, 4, 10, 1, 2, 0]);
        assert_eq!(
            Wire::Network { cidr: true }.decode(&buffer[..]),
            Ok("10.1.2.0/24".to_owned())
        );

        let mut buffer = BytesMut::new();
        SqlValParam(&SqlVal::Text("192.168.1.5".to_owned()))
            .to_sql_checked(&Type::INET, &mut buffer)
            .unwrap();
        assert_eq!(
            Wire::Network { cidr: false }.decode(&buffer[..]),
            Ok("192.168.1.5".to_owned())
        );
    }

    #[test]
    fn extension_enum_and_domain_columns_are_recognised() {
        let citext = synthetic("citext", 65_535, Kind::Simple);
        assert_eq!(wire_of(&citext), Some(Wire::Text));

        let labels = vec!["happy".to_owned(), "sad".to_owned()];
        let mood = synthetic("mood", 65_534, Kind::Enum(labels));
        assert_eq!(wire_of(&mood), Some(Wire::Text));

        let email = synthetic("email", 65_533, Kind::Domain(Type::TEXT));
        assert_eq!(wire_of(&email), Some(Wire::Text));

        let amount = synthetic("amount", 65_532, Kind::Domain(Type::NUMERIC));
        assert_eq!(wire_of(&amount), Some(Wire::Numeric));

        assert_eq!(wire_of(&Type::INET), Some(Wire::Network { cidr: false }));
        assert_eq!(wire_of(&Type::CIDR), Some(Wire::Network { cidr: true }));
        assert_eq!(wire_of(&Type::INTERVAL), None);
    }

    #[test]
    fn a_textual_column_accepts_its_string_representation() {
        let citext = synthetic("citext", 65_535, Kind::Simple);
        let mut buffer = BytesMut::new();
        assert!(matches!(
            SqlValParam(&SqlVal::Text("Mixed Case".to_owned()))
                .to_sql_checked(&citext, &mut buffer)
                .unwrap(),
            IsNull::No
        ));
        assert_eq!(buffer.to_vec(), b"Mixed Case".to_vec());
    }

    #[test]
    fn a_value_without_a_textual_form_is_rejected_for_a_network_column() {
        let mut buffer = BytesMut::new();
        let error = match SqlValParam(&SqlVal::I64(1)).to_sql_checked(&Type::INET, &mut buffer) {
            Ok(_) => panic!("an integer has no textual network address form"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("int64"));
        assert!(error.to_string().contains("inet"));
    }
}
