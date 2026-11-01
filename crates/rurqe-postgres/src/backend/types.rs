//! Shared parameter binding and row decoding for the PostgreSQL driver adapters.
//!
//! Both adapters hand their statements to the same binding layer: a [`SqlVal`]
//! parameter is encoded into the binary representation the server expects for
//! the column it targets, and a result column is decoded back into the ordered
//! [`SqlVal`]s RURQE rows are built from.

mod array;
mod compat;
mod driver;
mod fallback;
mod network;
mod numeric;
mod wire;

#[cfg(test)]
mod tests;

use std::error::Error as StdError;
use std::fmt;

use bytes::BytesMut;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::BuiltQuery;

use self::compat::value_fits;
use self::driver::types::{FromSql, IsNull, Kind, ToSql, Type};
use self::driver::{Error as DriverError, Row};
use self::wire::{base_type, wire_of, Wire};

/// Error raised while executing a query through a PostgreSQL driver adapter.
///
/// The enum is `#[non_exhaustive]`, because the failures it names follow the
/// adapters a build compiles: a match written against it carries a wildcard arm,
/// so a variant added for a new adapter does not break the matches already
/// compiled.
///
/// One variant follows the feature matrix directly. `PgError::Pool` exists only
/// under the `deadpool-postgres` feature, because the asynchronous pool is the
/// adapter that checks a connection out of a pool on its own. The shape of the
/// enum is therefore the shape of the adapters a build carries, and a downstream
/// match observes the variant set of its own build.
#[derive(Debug)]
#[non_exhaustive]
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
    /// The connection is poisoned and refuses to execute further statements.
    ///
    /// A poison record is written when the adapter can no longer prove that the
    /// connection is outside a transaction: a transaction-control statement the
    /// driver refused, or a transaction block that ended without one. PostgreSQL
    /// accepts a `BEGIN` inside an open transaction with a warning, so executing
    /// on such a connection would run the statement inside the abandoned
    /// transaction and persist it with the next `COMMIT`. The connection has to be
    /// discarded instead.
    ConnectionPoisoned {
        /// The transaction-control failure that poisoned the connection.
        reason: String,
    },
    /// A pooled checkout could not return the connection to an idle session.
    ///
    /// The reset a checkout performs ends whatever transaction the session of
    /// a recycled connection still held, so a driver failure during it leaves
    /// that session in a state the pool cannot vouch for. The connection is
    /// therefore not handed out; the failure the driver reported is rendered
    /// into `detail`, because the checkout that produced it is the only place
    /// that observed it.
    AbandonedTransaction {
        /// The driver failure the reset reported.
        detail: String,
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
            PgError::ConnectionPoisoned { reason } => {
                write!(f, "the PostgreSQL connection is poisoned: {reason}")
            }
            PgError::AbandonedTransaction { detail } => write!(
                f,
                "the PostgreSQL connection could not be returned to an idle session: {detail}"
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
            PgError::ConnectionPoisoned { .. } => None,
            PgError::AbandonedTransaction { .. } => None,
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
/// [`SqlVal`] variant: codegen maps them to `String`, so their text values are
/// parsed back into the type the column declares before being encoded. A build
/// without the feature backing one of those families encodes the text through the
/// featureless codec of its family — the inverse of the decoder that renders such
/// a column back into text — so the mapping the code generator produces reaches
/// the server in every build.
fn text_to_sql(
    value: &str,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    // The textual spelling of a value is the one of the type a domain is
    // declared over, so the domain chain is resolved before the value is parsed.
    let ty = base_type(ty);

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

    // A build without the crate backing one of those families encodes the text
    // through the featureless codec of its family, which is the inverse of the
    // decoder that renders such a column back into text. Covering both directions
    // with the same families keeps a value read from a column writable to it, so
    // the mapping the code generator produces reaches the server in every build.
    if let Some(encoded) = fallback::encode(ty, value, out) {
        encoded.map_err(|reason| encoding_failure(&reason))?;
        return Ok(IsNull::No);
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

/// The parameters of one statement, prepared once for a driver call.
///
/// A statement binds its parameters in two steps: every [`SqlVal`] is wrapped in
/// the [`SqlValParam`] that implements the driver's parameter trait, and the
/// wrapped values are handed to the driver as references to it. Both steps live
/// here, so every entry point of the adapters prepares its parameters through
/// one implementation, and a different buffer strategy — keeping the low
/// parameter counts on the stack, for instance — is applied in one place
/// instead of being replayed in each entry point.
pub(crate) struct BoundParams<'a> {
    /// The wrapper of every bound parameter, which the driver references.
    bound: Vec<SqlValParam<'a>>,
}

impl<'a> BoundParams<'a> {
    /// Prepares the parameters of `query`.
    pub(crate) fn of(query: &'a BuiltQuery) -> Self {
        Self {
            bound: query.params.iter().map(SqlValParam).collect(),
        }
    }

    /// Borrows the parameters as the references a driver call accepts.
    ///
    /// The driver takes the bound list as an iterator, so the references are
    /// yielded one at a time: collecting them into a buffer of their own would
    /// add a second allocation to every statement before it reaches the server.
    pub(crate) fn bindings(&self) -> impl ExactSizeIterator<Item = &(dyn ToSql + Sync)> + '_ {
        self.bound.iter().map(|param| param as &(dyn ToSql + Sync))
    }

    /// Collects the parameters into the reference buffer a slice-based driver
    /// call accepts.
    ///
    /// The statement API of the synchronous driver takes its parameters as one
    /// slice rather than as an iterator, so the references are collected into
    /// the single buffer that call requires. Every other driver call borrows
    /// the wrappers through [`bindings`](BoundParams::bindings) and allocates
    /// nothing beyond the buffer of wrappers itself.
    pub(crate) fn references(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.bindings().collect()
    }
}

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

        // The binary layout of a value follows the type its column is declared
        // over: a domain carries the representation of that type. Resolving the
        // domain chain here keeps a domain column binding the same octets as the
        // type it is declared over instead of reaching the server under a layout
        // of its own.
        let column_ty = base_type(ty);

        match wire_of(column_ty) {
            // Textual columns and plain scalars keep the per-value encoding.
            None | Some(Wire::Text) => {}
            // A column the code generator maps onto `String` has no `SqlVal`
            // variant of its own, so the textual value is encoded into the binary
            // representation the server expects for the column type.
            Some(wire) => return encode_wire(wire, self.0, ty, out),
        }

        // The column whitelist decides whether a parameter may be bound at all,
        // not whether this value belongs in that column: a value of an unrelated
        // kind would otherwise reach the wire and be reported by the server as an
        // encoding failure. Refusing it here keeps the failure in the diagnostic
        // of the adapter itself and leaves the buffer untouched.
        if !value_fits(column_ty, self.0) {
            return Err(unusable_value(self.0, ty));
        }

        match self.0 {
            SqlVal::Null => Ok(IsNull::Yes),
            SqlVal::Bool(value) => value.to_sql(column_ty, out),
            SqlVal::I32(value) => encode_integer(i64::from(*value), self.0, column_ty, out),
            SqlVal::I64(value) => encode_integer(*value, self.0, column_ty, out),
            SqlVal::F64(value) => encode_float(*value, self.0, column_ty, out),
            SqlVal::Text(value) => text_to_sql(value, column_ty, out),
            SqlVal::Bytes(value) => value.as_slice().to_sql(column_ty, out),
            #[cfg(feature = "decimal")]
            SqlVal::Decimal(decimal) => decimal_to_sql(*decimal, self.0, column_ty, out),
            #[cfg(feature = "uuid")]
            SqlVal::Uuid(value) => {
                if column_ty == &Type::UUID {
                    value.to_sql(column_ty, out)
                } else {
                    value.to_string().to_sql(column_ty, out)
                }
            }
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDateTime(value) => {
                if column_ty == &Type::TIMESTAMP {
                    value.to_sql(column_ty, out)
                } else if column_ty == &Type::TIMESTAMPTZ {
                    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(*value, chrono::Utc)
                        .to_sql(column_ty, out)
                } else {
                    value.to_string().to_sql(column_ty, out)
                }
            }
            #[cfg(feature = "chrono")]
            SqlVal::DateTimeUtc(value) => {
                if column_ty == &Type::TIMESTAMPTZ {
                    value.to_sql(column_ty, out)
                } else if column_ty == &Type::TIMESTAMP {
                    value.naive_utc().to_sql(column_ty, out)
                } else {
                    value.to_rfc3339().to_sql(column_ty, out)
                }
            }
            #[cfg(feature = "chrono")]
            SqlVal::NaiveDate(value) => {
                if column_ty == &Type::DATE {
                    value.to_sql(column_ty, out)
                } else {
                    value.to_string().to_sql(column_ty, out)
                }
            }
            #[cfg(feature = "serde_json")]
            SqlVal::Json(value) => {
                if column_ty == &Type::JSON || column_ty == &Type::JSONB {
                    value.to_sql(column_ty, out)
                } else {
                    value.to_string().to_sql(column_ty, out)
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
///
/// An exact decimal reaches the codec through its textual form, which is what
/// keeps every digit of the value in the binary payload instead of routing it
/// through a floating point number.
fn decimal_text(value: &SqlVal) -> Option<String> {
    match value {
        SqlVal::Text(text) => Some(text.clone()),
        SqlVal::I32(number) => Some(number.to_string()),
        SqlVal::I64(number) => Some(number.to_string()),
        SqlVal::F64(number) => Some(number.to_string()),
        #[cfg(feature = "decimal")]
        SqlVal::Decimal(decimal) => Some(decimal.to_string()),
        _ => None,
    }
}

/// Encodes an exact decimal into the representation of its column.
///
/// A `NUMERIC` column was already encoded by the wire codec before this point, so
/// the remaining columns are the ones a decimal reaches by conversion: an integer
/// column receives the truncated value at its own width, a `REAL` column the
/// closest single precision number, and every other column the textual form of the
/// value, exactly as a text parameter would reach it.
#[cfg(feature = "decimal")]
fn decimal_to_sql(
    decimal: rust_decimal::Decimal,
    value: &SqlVal,
    column_ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    // The conversion into a floating point number is provided by the primitive
    // conversion trait rather than by an inherent method of the decimal type, so
    // the trait is brought into scope for the two conversion arms below.
    use rust_decimal::prelude::ToPrimitive;

    if column_ty == &Type::INT2 || column_ty == &Type::INT4 || column_ty == &Type::INT8 {
        let number = decimal_integer(&decimal).ok_or_else(|| out_of_range(value, column_ty))?;
        return encode_integer(number, value, column_ty, out);
    }
    if column_ty == &Type::FLOAT4 || column_ty == &Type::FLOAT8 {
        let number = decimal
            .to_f64()
            .ok_or_else(|| out_of_range(value, column_ty))?;
        return encode_float(number, value, column_ty, out);
    }

    decimal.to_string().to_sql(column_ty, out)
}

/// Renders the integer part of an exact decimal.
///
/// The mantissa carries every digit of the value scaled by `10^scale`, so the
/// integer part is the mantissa divided by that scale. The division runs in a 128
/// bit integer, which holds both the mantissa and the largest scale a decimal can
/// declare, and a value outside the 64 bit range reports `None` rather than
/// wrapping around.
#[cfg(feature = "decimal")]
fn decimal_integer(decimal: &rust_decimal::Decimal) -> Option<i64> {
    let divisor = 10i128.checked_pow(decimal.scale())?;
    i64::try_from(decimal.mantissa() / divisor).ok()
}

/// Encodes an integer at the width of the column it is bound to.
///
/// The payload width follows the column rather than the value: a `SMALLINT`
/// column reads two octets and a `BIGINT` column eight, so a value that reaches a
/// column of another width is converted first instead of sending a payload the
/// server would read as a different number.
fn encode_integer(
    number: i64,
    value: &SqlVal,
    column_ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    if column_ty == &Type::INT2 {
        let narrowed = i16::try_from(number).map_err(|_| out_of_range(value, column_ty))?;
        return narrowed.to_sql(column_ty, out);
    }
    if column_ty == &Type::INT4 {
        let narrowed = i32::try_from(number).map_err(|_| out_of_range(value, column_ty))?;
        return narrowed.to_sql(column_ty, out);
    }

    number.to_sql(column_ty, out)
}

/// Encodes a floating point value at the width of the column it is bound to.
///
/// A `REAL` column carries four octets, so a value bound from a 64 bit float is
/// narrowed to the single precision range the column holds.
fn encode_float(
    number: f64,
    value: &SqlVal,
    column_ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    if column_ty == &Type::FLOAT4 {
        let narrowed = narrow_to_f32(number).ok_or_else(|| out_of_range(value, column_ty))?;
        return narrowed.to_sql(column_ty, out);
    }

    number.to_sql(column_ty, out)
}

/// Narrows a 64 bit float to the single precision range.
///
/// `f32` declares no fallible conversion from `f64`, so the overflow is detected
/// by comparing the finiteness of the value with the finiteness of its narrowed
/// form: a finite value the single precision range cannot hold narrows into an
/// infinity the column never asked for. An infinite or `NaN` value keeps its own
/// kind, which a `REAL` column represents faithfully.
fn narrow_to_f32(number: f64) -> Option<f32> {
    let narrowed = number as f32;
    (narrowed.is_finite() == number.is_finite()).then_some(narrowed)
}

/// Describes a value outside the range of a column of type `ty`.
fn out_of_range(value: &SqlVal, ty: &Type) -> Box<dyn StdError + Sync + Send> {
    Box::new(::std::io::Error::other(format!(
        "a SQL `{}` value is out of the range of a PostgreSQL column of type `{}`",
        value.variant_name(),
        ty.name()
    )))
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

/// Binary payload of a result column, borrowed from the row carrying it.
///
/// The driver decodes a column only into a type that declares itself compatible
/// with the column type, which leaves the values RURQE reads through its own
/// codecs — arrays, decimals, network addresses, enum labels, extension types,
/// and domains over any of them — out of reach. Reading the payload as borrowed
/// bytes lets the adapter decode it itself without copying the column first.
struct RawValue<'a>(&'a [u8]);

impl<'a> FromSql<'a> for RawValue<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn StdError + Sync + Send>> {
        Ok(RawValue(raw))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// Classified decoding rules of a result column.
///
/// Classifying a column type resolves its domain chain and compares type names,
/// which yields the same answer for every row a statement returns. Resolving it
/// once per column keeps that work off the path taken for every decoded value,
/// and owning the resolved type keeps the classification valid for every row of
/// the result set rather than for the row it was read from.
struct ColumnPlan {
    /// The type the column is declared over, with its domain chain resolved.
    base: Type,
    /// Textual representation of the column type, when it declares one.
    wire: Option<Wire>,
}

impl ColumnPlan {
    /// Classifies a column declared as `ty`.
    fn of(ty: &Type) -> Self {
        let base = base_type(ty).clone();
        let wire = wire_of(&base);
        Self { base, wire }
    }
}

/// A row of a result set, as the decoding layer reads it.
///
/// A driver hands its own row type to the adapters, and every row reports the
/// type each of its columns is declared as together with the binary payload that
/// column carries. Reading a row through this view keeps the decoding loop
/// independent of how a driver materialises one, which is what lets the loop run
/// against a synthetic row in the test suite without a running server.
pub(crate) trait ResultRow {
    /// Types the columns of the row are declared as, in projection order.
    fn column_types(&self) -> impl Iterator<Item = &Type> + '_;

    /// Binary payload of the column at `index`, or `None` for SQL `NULL`.
    fn column_payload(&self, index: usize) -> Result<Option<&[u8]>, PgError>;
}

impl ResultRow for Row {
    fn column_types(&self) -> impl Iterator<Item = &Type> + '_ {
        self.columns().iter().map(|column| column.type_())
    }

    fn column_payload(&self, index: usize) -> Result<Option<&[u8]>, PgError> {
        let raw: Option<RawValue<'_>> = self.try_get(index)?;
        Ok(raw.map(|raw| raw.0))
    }
}

/// Decodes the rows of a result set one at a time.
///
/// Every row a statement returns carries the same columns, so the decoding rules
/// of the result set are classified once, from the first row that arrives, and
/// reused for every further row. Decoding a row on arrival keeps the message
/// buffer of one row alive at a time instead of holding the whole result set in
/// its raw form while its decoded values are built, so a fetch never materialises
/// its result set twice.
pub(crate) struct RowDecoder {
    /// Decoding rules of the result set, classified from its first row.
    plans: Option<Vec<ColumnPlan>>,
}

impl RowDecoder {
    /// Creates a decoder that has not read a row of its result set yet.
    pub(crate) fn new() -> Self {
        Self { plans: None }
    }

    /// Decodes `row`, classifying the result set from the first row it receives.
    pub(crate) fn decode<R: ResultRow>(&mut self, row: &R) -> Result<Vec<SqlVal>, PgError> {
        let plans = self.plans.get_or_insert_with(|| classify(row));
        decode_row(plans, row)
    }
}

/// Classifies the decoding rules of a result set from one of its rows.
fn classify<R: ResultRow>(row: &R) -> Vec<ColumnPlan> {
    row.column_types().map(ColumnPlan::of).collect()
}

/// Decodes one row with the plans classified for its result set.
fn decode_row<R: ResultRow>(plans: &[ColumnPlan], row: &R) -> Result<Vec<SqlVal>, PgError> {
    let mut values = Vec::with_capacity(plans.len());

    for (index, declared) in row.column_types().enumerate() {
        let decoded = match row.column_payload(index)? {
            Some(payload) => match plans.get(index) {
                Some(plan) => decode_payload(&plan.base, plan.wire, payload),
                // A row of another shape keeps decoding by classifying its own
                // column instead of being reported as an unsupported type.
                None => decode_value(declared, payload),
            },
            None => Ok(SqlVal::Null),
        };

        values.push(decoded.map_err(|failure| column_error(failure, declared))?);
    }

    Ok(values)
}

/// Describes a column payload that cannot be decoded into a [`SqlVal`].
fn column_error(failure: DecodeError, column_ty: &Type) -> PgError {
    match failure {
        DecodeError::Unsupported => PgError::UnsupportedType(column_ty.name().to_owned()),
        DecodeError::Invalid(reason) => PgError::InvalidValue {
            column_type: column_ty.name().to_owned(),
            reason,
        },
    }
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

/// Decodes the binary payload of one value of the declared type `ty`.
///
/// A column holding an array is decoded element by element, and every other
/// column is decoded directly, so a column and an element of an array of the same
/// type share one codec. Classifying the type here is what keeps an array element
/// — which reaches this entry point without a plan of its own — decoding exactly
/// like the column carrying it.
fn decode_value(ty: &Type, payload: &[u8]) -> Result<SqlVal, DecodeError> {
    let plan = ColumnPlan::of(ty);
    decode_payload(&plan.base, plan.wire, payload)
}

/// Decodes the binary payload of a value whose column was already classified.
fn decode_payload(base: &Type, wire: Option<Wire>, payload: &[u8]) -> Result<SqlVal, DecodeError> {
    if let Kind::Array(elem_ty) = base.kind() {
        let elements = array::decode(elem_ty, payload).map_err(DecodeError::Invalid)?;
        return Ok(SqlVal::Array(elements));
    }

    scalar_value(base, wire, payload)
}

/// Decodes the binary payload of a value that is not an array.
fn scalar_value(ty: &Type, wire: Option<Wire>, payload: &[u8]) -> Result<SqlVal, DecodeError> {
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
        return extended_value(ty, wire, payload);
    };

    Ok(value)
}

/// Decodes a value whose type is recognised through its family rather than
/// through one of the primitive scalars.
fn extended_value(ty: &Type, wire: Option<Wire>, payload: &[u8]) -> Result<SqlVal, DecodeError> {
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

    // An exact decimal column keeps every digit of its value instead of passing
    // through the textual form a column mapped onto `String` carries: the payload
    // already renders the decimal text, which is parsed back into the exact type.
    // A payload the exact type cannot represent, such as the `NaN` sentinel, keeps
    // its textual form.
    #[cfg(feature = "decimal")]
    if ty == &Type::NUMERIC {
        if let Ok(text) = Wire::Numeric.decode(payload) {
            if let Ok(decimal) = text.parse::<rust_decimal::Decimal>() {
                return Ok(SqlVal::Decimal(decimal));
            }
        }
    }

    // A build without the crate backing a value family still reads its columns:
    // the payload is rendered in the textual form a column the code generator
    // mapped onto `String` carries.
    if let Some(text) = fallback::decode(ty, payload) {
        return text.map(SqlVal::Text).map_err(DecodeError::Invalid);
    }

    // A type outside the native scalars is decoded by the wire codec naming its
    // textual representation, which is also what makes an enum label, an
    // extension type, or a domain over a native type readable. The plan of the
    // column carries that classification, so it is not resolved a second time.
    match wire {
        Some(representation) => representation
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
