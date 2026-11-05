//! Nested projections: a related row set aggregated into a single JSON value.
//!
//! A one-to-many relation is read in one statement instead of one statement per
//! parent: the engine aggregates the child rows of every parent into a JSON
//! array, and the array is decoded back into the row type of the child
//! projection. A one-to-one relation uses the same aggregation stopped at its
//! first row, so a missing child decodes into `None` instead of an empty list.
//!
//! The document is produced by the engine, which keeps the ordering, the row
//! limit, and the filter of the nested statement in charge of its contents:
//!
//! ```text
//! SELECT "id",
//!        COALESCE((SELECT jsonb_agg(to_jsonb("__rurqe_sub"))
//!                  FROM (SELECT "sku" AS "sku", "qty" AS "qty" FROM "lines")
//!                  AS "__rurqe_sub"), '[]'::jsonb)
//! FROM "orders"
//! ```
//!
//! PostgreSQL aggregates natively through `jsonb_agg`, while a dialect without
//! native row aggregation — see [`Dialect::SUPPORTS_JSONB_ROW_AGG`] — assembles
//! every object with `json_object` and collects the objects with
//! `json_group_array`. Both forms produce the same JSON document, so the client
//! decodes one representation whichever engine produced it.

use crate::dialect::{Dialect, SupportsJsonAggregation};
use crate::expr::{DialectInScope, ExprKind, SqlArrayElement, SqlVal, Subquery};
use crate::query::{
    GroupedQuery, HasProjection, SelectQuery, SelectTarget, SelectTargetInScope, TableScope,
};
use crate::schema::At;

/// The row type a nested projection decodes into.
///
/// The rows of a nested collection are one JSON object each, so the row type
/// declares the keys its fields are read from in projection order. The trait
/// extends [`SqlArrayElement`] because a collection decodes through the same
/// list contracts an array column uses: the JSON array of the engine becomes a
/// `Vec<Row>`, and the absent row of a one-to-one relation becomes `None`.
///
/// `FromSqlValue` of an implementor receives the JSON object of one row — as
/// [`SqlVal::Json`] from PostgreSQL or as the JSON text of [`SqlVal::Text`] from
/// SQLite — and [`json_object`] turns either form into the object it reads.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be projected as a nested row",
    label = "unsupported nested row type",
    note = "a nested row type implements `MultisetRow` next to `ToSqlVal`, `FromSqlValue`, and `SqlArrayElement`"
)]
pub trait MultisetRow: SqlArrayElement + 'static {
    /// The JSON keys of the projected fields, in projection order.
    const FIELDS: &'static [&'static str];
}

/// A nested projection producing every row of the wrapped query.
///
/// The projection is read as `Vec<Row>`: a parent without children yields an
/// empty vector rather than a missing field.
pub struct NestMany<Q>(pub Q);

/// A nested projection producing at most one row of the wrapped query.
///
/// The projection is read as `Option<Row>`: a parent without a matching child
/// yields `None`, so the absence stays explicit instead of being decoded from a
/// sentinel row.
pub struct NestOne<Q>(pub Q);

/// Wraps `query` into a projection producing its rows as a nested collection.
pub fn nest_many<Q>(query: Q) -> NestMany<Q> {
    NestMany(query)
}

/// Wraps `query` into a projection producing at most one of its rows.
pub fn nest_one<Q>(query: Q) -> NestOne<Q> {
    NestOne(query)
}

/// Builds the nested collection node of the statement `query`.
///
/// The statement is taken as plain data, so a plain `SELECT` and a grouped one
/// are embedded through the same node and decode into the same row type.
fn nested_kind<Row: MultisetRow>(query: Subquery, single: bool) -> ExprKind {
    ExprKind::Multiset {
        query,
        fields: Row::FIELDS.to_vec(),
        single,
    }
}

impl<D, S, Row> SelectTarget for NestMany<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Vec<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind::<Row>(Subquery::new(self.0.to_ast()), false)
    }
}

impl<D, S, Row, G> SelectTarget for NestMany<GroupedQuery<D, S, Row, G, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Vec<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind::<Row>(Subquery::new(self.0.to_ast()), false)
    }
}

// A nested projection reads from its own statement, so it references no table of
// the enclosing scope and resolves at the first position of every scope. Fixing
// the position keeps the marker determined instead of leaving inference to pick
// one of the positions a scope may offer.
//
// The relationship itself is a relational one, so the nested statement belongs to
// the same dialect as the statement projecting it, and that dialect has to render
// the JSON aggregation the document is assembled with. A dialect without the
// aggregation would accept the projection and fail inside the engine instead.
impl<Tables, InnerD, QueryD, S, Row> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestMany<SelectQuery<InnerD, S, Row, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Vec<Row>;
}

// A grouped statement is projected like a plain one: it reads from its own
// relations, so it resolves at the same fixed position, and it has to be
// rendered by a dialect with the JSON aggregation the document is assembled with.
impl<Tables, InnerD, QueryD, S, Row, G> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestMany<GroupedQuery<InnerD, S, Row, G, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Vec<Row>;
}

impl<D, S, Row> SelectTarget for NestOne<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Option<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind::<Row>(Subquery::new(self.0.to_ast()), true)
    }
}

impl<D, S, Row, G> SelectTarget for NestOne<GroupedQuery<D, S, Row, G, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Option<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind::<Row>(Subquery::new(self.0.to_ast()), true)
    }
}

// A single nested row reads from its own statement as well, so it resolves at the
// same fixed position as the collection form, and the statement projecting it
// carries the same dialect and aggregation requirements.
impl<Tables, InnerD, QueryD, S, Row> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestOne<SelectQuery<InnerD, S, Row, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Option<Row>;
}

impl<Tables, InnerD, QueryD, S, Row, G> SelectTargetInScope<Tables, At<0>, QueryD>
    for NestOne<GroupedQuery<InnerD, S, Row, G, HasProjection>>
where
    InnerD: Dialect + DialectInScope<QueryD>,
    QueryD: SupportsJsonAggregation,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Option<Row>;
}

/// The exact value of a JSON number the integer representation cannot carry.
///
/// PostgreSQL aggregates a `NUMERIC` column into a nested document as a JSON
/// number, and the digits identifying that value live in the textual form of the
/// number. Reading them back into an exact decimal keeps the value the column
/// declares, where a floating point representation would round it.
///
/// The conversion reports `None` for a number the decimal type cannot hold, so
/// the caller falls back to the next representation of the value.
#[cfg(all(feature = "serde_json", feature = "decimal"))]
fn exact_json_number(number: &serde_json::Number) -> Option<SqlVal> {
    number
        .to_string()
        .parse::<rust_decimal::Decimal>()
        .ok()
        .map(SqlVal::Decimal)
}

/// The exact value of a JSON number the integer representation cannot carry.
///
/// A build without the `decimal` feature carries no exact decimal value, so the
/// number keeps the floating point representation it was parsed with.
#[cfg(all(feature = "serde_json", not(feature = "decimal")))]
fn exact_json_number(_number: &serde_json::Number) -> Option<SqlVal> {
    None
}

/// Converts one JSON number into the SQL value carrying it.
///
/// An integer stays an integer, and a number the integer representation cannot
/// carry — a fraction, or an integer wider than the signed 64-bit range — keeps
/// every digit of its textual form whenever an exact decimal is compiled in.
#[cfg(feature = "serde_json")]
fn json_number_to_sql_value(number: &serde_json::Number) -> SqlVal {
    if let Some(value) = number.as_i64() {
        return SqlVal::I64(value);
    }

    if let Some(value) = exact_json_number(number) {
        return value;
    }

    match number.as_f64() {
        Some(value) => SqlVal::F64(value),
        None => SqlVal::Text(number.to_string()),
    }
}

/// Converts a JSON value of a nested document into the matching SQL value.
///
/// Scalars keep their natural representation, so a field of a nested row decodes
/// through exactly the same [`FromSqlValue`](crate::row::FromSqlValue) contract a
/// column of the same type decodes through: a binary field reads its canonical
/// hexadecimal text back into the bytes it carries. A number wider than the
/// integer representation, or one carrying a fraction, keeps its exact digits
/// when the `decimal` feature is compiled in, so a `NUMERIC` column of a nested
/// row never passes through a floating point approximation.
#[cfg(feature = "serde_json")]
pub fn json_to_sql_value(value: &serde_json::Value) -> SqlVal {
    match value {
        serde_json::Value::Null => SqlVal::Null,
        serde_json::Value::Bool(value) => SqlVal::Bool(*value),
        serde_json::Value::Number(number) => json_number_to_sql_value(number),
        serde_json::Value::String(text) => SqlVal::Text(text.clone()),
        other => SqlVal::Json(other.clone()),
    }
}

/// Converts a SQL value back into the JSON representation of a nested document.
///
/// The function is the inverse of [`json_to_sql_value`], so a row type that reads
/// its fields with [`nested_field`] writes them back into the object of one row:
/// the document bound as a parameter and the document decoded from a result row
/// carry the same values whichever engine produced them.
///
/// A value of a kind a JSON document cannot carry — a UUID or a temporal value —
/// keeps the text it is displayed with, which is the scalar form
/// [`FromSqlValue`](crate::row::FromSqlValue) parses back into the same type. A
/// binary value carries its canonical hexadecimal text instead, which
/// [`FromSqlValue`](crate::row::FromSqlValue) for `Vec<u8>` parses back into the
/// same bytes, so a binary field of a nested row survives the round trip rather
/// than keeping a debug rendering no decoder accepts. An exact decimal becomes
/// a JSON number while that number keeps every digit of it, and the same text
/// otherwise. A floating point value JSON cannot represent, `NaN` and the two
/// infinities, becomes `null` instead of failing the encoding.
#[cfg(feature = "serde_json")]
pub fn sql_val_to_json(value: &SqlVal) -> serde_json::Value {
    match value {
        SqlVal::Null => serde_json::Value::Null,
        SqlVal::Bool(flag) => serde_json::Value::Bool(*flag),
        SqlVal::I32(number) => serde_json::Value::from(*number),
        SqlVal::I64(number) => serde_json::Value::from(*number),
        SqlVal::F64(number) => json_number(*number),
        #[cfg(feature = "decimal")]
        SqlVal::Decimal(value) => json_decimal(*value),
        SqlVal::Text(text) => serde_json::Value::String(text.clone()),
        // A binary value carries its canonical hexadecimal text instead of the
        // debug rendering of its vector: `FromSqlValue` for `Vec<u8>` decodes
        // that text back into the same bytes, so a binary field of a nested row
        // round trips through the document.
        SqlVal::Bytes(bytes) => serde_json::Value::String(crate::row::hex_encode(bytes)),
        SqlVal::Json(document) => document.clone(),
        SqlVal::Array(values) => {
            serde_json::Value::Array(values.iter().map(sql_val_to_json).collect())
        }
        other => serde_json::Value::String(other.to_string()),
    }
}

/// Builds the JSON number of `value`, reporting a value JSON cannot carry as
/// `null` instead of failing the encoding.
#[cfg(feature = "serde_json")]
fn json_number(value: f64) -> serde_json::Value {
    match serde_json::Number::from_f64(value) {
        Some(number) => serde_json::Value::Number(number),
        None => serde_json::Value::Null,
    }
}

/// Builds the JSON representation of an exact decimal value.
///
/// A decimal a JSON number carries with every digit preserved is written as that
/// number, so the object of a nested row has the shape the engine produces for a
/// `NUMERIC` column. A value the number cannot carry exactly — its digit string
/// does not survive the floating point form of a JSON number — is written as text
/// instead, which [`json_to_sql_value`] reads back into the same exact value.
#[cfg(all(feature = "serde_json", feature = "decimal"))]
fn json_decimal(value: rust_decimal::Decimal) -> serde_json::Value {
    let text = value.to_string();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Number(number))
            if number.to_string().parse::<rust_decimal::Decimal>().ok() == Some(value) =>
        {
            serde_json::Value::Number(number)
        }
        _ => serde_json::Value::String(text),
    }
}

/// The JSON object of one nested row.
///
/// The object stays opaque: a nested row is read through [`nested_field`], which
/// turns it into the values the declared fields name. Keeping one type across
/// builds lets [`json_object`], [`nested_field`], and [`multiset_object`] stay
/// callable whether or not a JSON crate is compiled in, so the projections
/// synthesized by `dto!` and `select!` resolve their paths in every build.
#[cfg(feature = "serde_json")]
#[derive(Debug, Clone)]
pub struct RowObject(serde_json::Map<String, serde_json::Value>);

#[cfg(feature = "serde_json")]
impl RowObject {
    /// Reads the JSON value stored under `field`, if the object carries it.
    pub fn get(&self, field: &str) -> Option<&serde_json::Value> {
        self.0.get(field)
    }
}

/// The JSON object of one nested row.
///
/// A build without the `serde_json` feature carries no JSON document, so the
/// object holds nothing and every read reports the missing feature. The type and
/// the helpers around it stay callable, which is what keeps a `dto!` or `select!`
/// expansion compiling without the feature.
#[cfg(not(feature = "serde_json"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowObject;

/// The diagnostic a nested row reports in a build without the `serde_json` feature.
#[cfg(not(feature = "serde_json"))]
fn json_support_missing(operation: &str) -> crate::row::RowDecodeError {
    crate::row::RowDecodeError::InvalidValue {
        target: "a nested row",
        found: "json",
        detail: format!("{operation} requires the `serde_json` feature of `rurqe-core`"),
    }
}

/// Builds the JSON object of a nested row from the values of its fields.
///
/// The keys are the field names the row declares in
/// [`MultisetRow::FIELDS`](crate::nested::MultisetRow), so a row written through
/// this helper decodes back through [`nested_field`] and carries exactly the keys
/// the engine writes for a projected collection.
#[cfg(feature = "serde_json")]
pub fn multiset_object(fields: &[(&str, SqlVal)]) -> SqlVal {
    let mut object = serde_json::Map::with_capacity(fields.len());
    for (name, value) in fields {
        object.insert((*name).to_owned(), sql_val_to_json(value));
    }
    SqlVal::Json(serde_json::Value::Object(object))
}

/// Builds the JSON object of a nested row from the values of its fields.
///
/// A build without the `serde_json` feature cannot write a JSON document, so the
/// value reports the missing feature as an encoding error: the statement still
/// renders, and the driver surfaces the reason instead of binding an empty row.
#[cfg(not(feature = "serde_json"))]
pub fn multiset_object(_fields: &[(&str, SqlVal)]) -> SqlVal {
    SqlVal::EncodingError(
        "writing a nested row requires the `serde_json` feature of `rurqe-core`".to_owned(),
    )
}

/// Parses the JSON object a nested document holds.
///
/// PostgreSQL reports the object of a nested row as a JSON value, while SQLite
/// reports the same document as text, so both forms are accepted here.
#[cfg(feature = "serde_json")]
pub fn json_object(value: &SqlVal) -> Result<RowObject, crate::row::RowDecodeError> {
    use crate::row::RowDecodeError;

    let mismatch = || RowDecodeError::TypeMismatch {
        target: "a nested row",
        found: value.variant_name(),
    };

    match value {
        SqlVal::Json(serde_json::Value::Object(object)) => Ok(RowObject(object.clone())),
        SqlVal::Json(_) => Err(mismatch()),
        SqlVal::Text(text) => match serde_json::from_str(text) {
            Ok(serde_json::Value::Object(object)) => Ok(RowObject(object)),
            Ok(_) => Err(mismatch()),
            Err(error) => Err(RowDecodeError::InvalidValue {
                target: "a nested row",
                found: "text",
                detail: error.to_string(),
            }),
        },
        _ => Err(mismatch()),
    }
}

/// Parses the JSON object a nested document holds.
///
/// A build without the `serde_json` feature carries no JSON document, so reading
/// one reports the missing feature. The function stays callable, which is what
/// keeps a `dto!` or `select!` expansion compiling without the feature.
#[cfg(not(feature = "serde_json"))]
pub fn json_object(_value: &SqlVal) -> Result<RowObject, crate::row::RowDecodeError> {
    Err(json_support_missing("reading a nested row"))
}

/// Decodes the field `field` of a nested JSON object into `T`.
///
/// A missing key is reported instead of decoding into a default value, so the
/// mismatch between the projected and the declared fields surfaces where the row
/// is read.
#[cfg(feature = "serde_json")]
pub fn nested_field<T: crate::row::FromSqlValue>(
    object: &RowObject,
    field: &str,
) -> Result<T, crate::row::RowDecodeError> {
    match object.get(field) {
        Some(value) => T::from_sql_value(&json_to_sql_value(value)),
        None => Err(crate::row::RowDecodeError::InvalidValue {
            target: std::any::type_name::<T>(),
            found: "json",
            detail: format!("the nested row carries no `{field}` field"),
        }),
    }
}

/// Decodes the field `field` of a nested JSON object into `T`.
///
/// A build without the `serde_json` feature reads no JSON document, so decoding a
/// field reports the missing feature. The function stays callable, which is what
/// keeps a `dto!` or `select!` expansion compiling without the feature.
#[cfg(not(feature = "serde_json"))]
pub fn nested_field<T: crate::row::FromSqlValue>(
    _object: &RowObject,
    field: &str,
) -> Result<T, crate::row::RowDecodeError> {
    Err(json_support_missing(&format!(
        "reading the `{field}` field of a nested row"
    )))
}

#[cfg(all(test, feature = "serde_json"))]
mod tests;

#[cfg(all(test, not(feature = "serde_json")))]
mod without_json_tests;
