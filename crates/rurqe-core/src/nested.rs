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
use crate::query::{HasProjection, SelectQuery, SelectTarget, SelectTargetInScope, TableScope};
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
fn nested_kind<D, S, Row>(query: &SelectQuery<D, S, Row, HasProjection>, single: bool) -> ExprKind
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    ExprKind::Multiset {
        query: Subquery::new(query.to_ast()),
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
        nested_kind(&self.0, false)
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

impl<D, S, Row> SelectTarget for NestOne<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type RustType = Option<Row>;

    fn to_expr_kind(&self) -> ExprKind {
        nested_kind(&self.0, true)
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
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::expr::ToSqlVal;
    use crate::row::{FromSqlValue, RowDecodeError};
    use crate::schema::At;

    struct PgMock;

    impl Dialect for PgMock {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

    impl SupportsJsonAggregation for PgMock {}

    struct SqliteMock;

    impl Dialect for SqliteMock {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, _index: usize) {
            out.push('?');
        }
    }

    impl SupportsJsonAggregation for SqliteMock {}

    crate::table! {
        Owner => "owners" {
            OwnerId => "id": i64,
        }
    }

    crate::table! {
        Line => "lines" {
            LineSku => "sku": String,
            LineQty => "qty": i64,
        }
    }

    /// The child row a nested projection decodes into.
    type OrderLine = (String, i64);

    impl ToSqlVal for OrderLine {
        fn to_sql_val(self) -> SqlVal {
            multiset_object(&[
                ("sku", SqlVal::Text(self.0)),
                ("qty", SqlVal::I64(self.1)),
            ])
        }
    }

    impl FromSqlValue for OrderLine {
        fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
            let object = json_object(value)?;
            Ok((nested_field(&object, "sku")?, nested_field(&object, "qty")?))
        }
    }

    impl SqlArrayElement for OrderLine {}

    impl MultisetRow for OrderLine {
        const FIELDS: &'static [&'static str] = &["sku", "qty"];
    }

    /// The child row a nested projection decodes into, carrying a binary field.
    #[derive(Debug, Clone, PartialEq)]
    struct Document {
        name: String,
        blob: Vec<u8>,
    }

    impl ToSqlVal for Document {
        fn to_sql_val(self) -> SqlVal {
            multiset_object(&[
                ("name", SqlVal::Text(self.name)),
                ("blob", SqlVal::Bytes(self.blob)),
            ])
        }
    }

    impl FromSqlValue for Document {
        fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
            let object = json_object(value)?;
            Ok(Document {
                name: nested_field(&object, "name")?,
                blob: nested_field(&object, "blob")?,
            })
        }
    }

    impl SqlArrayElement for Document {}

    impl MultisetRow for Document {
        const FIELDS: &'static [&'static str] = &["name", "blob"];
    }

    /// The child row a nested projection decodes into, carrying a float field.
    #[derive(Debug, Clone, PartialEq)]
    struct PricedLine {
        sku: String,
        total: f64,
    }

    impl ToSqlVal for PricedLine {
        fn to_sql_val(self) -> SqlVal {
            multiset_object(&[
                ("sku", SqlVal::Text(self.sku)),
                ("total", SqlVal::F64(self.total)),
            ])
        }
    }

    impl FromSqlValue for PricedLine {
        fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
            let object = json_object(value)?;
            Ok(PricedLine {
                sku: nested_field(&object, "sku")?,
                total: nested_field(&object, "total")?,
            })
        }
    }

    impl SqlArrayElement for PricedLine {}

    impl MultisetRow for PricedLine {
        const FIELDS: &'static [&'static str] = &["sku", "total"];
    }

    /// The nested statement of the child rows of every owner.
    fn lines<D: Dialect>() -> SelectQuery<D, Line, OrderLine, HasProjection> {
        SelectQuery::new().from::<Line>().select((LineSku, LineQty))
    }

    #[test]
    fn a_nested_projection_carries_the_json_aggregation_requirement() {
        fn assert_projectable<Q, D>()
        where
            Q: SelectTargetInScope<Owner, At<0>, D>,
        {
        }

        assert_projectable::<
            NestMany<SelectQuery<PgMock, Line, OrderLine, HasProjection>>,
            PgMock,
        >();
        assert_projectable::<
            NestOne<SelectQuery<SqliteMock, Line, OrderLine, HasProjection>>,
            SqliteMock,
        >();
        assert_projectable::<
            NestMany<SelectQuery<PgMock, Line, OrderLine, HasProjection>>,
            PgMock,
        >();

        // The wrapped statement keeps rendering the rows it projects.
        let built = lines::<PgMock>().build().expect("valid lines query");
        assert_eq!(built.sql, "SELECT \"sku\", \"qty\" FROM \"lines\"");
        assert!(built.params.is_empty());
    }

    #[test]
    fn a_nested_projection_renders_its_row_type() {
        fn assert_row_type<Q: SelectTarget<RustType = Vec<OrderLine>>>() {}

        fn assert_single_row_type<Q: SelectTarget<RustType = Option<OrderLine>>>() {}

        assert_row_type::<NestMany<SelectQuery<PgMock, Line, OrderLine, HasProjection>>>();
        assert_single_row_type::<NestOne<SelectQuery<PgMock, Line, OrderLine, HasProjection>>>();
    }

    #[test]
    fn a_nested_collection_decodes_into_its_rows() {
        let document = serde_json::json!([
            { "sku": "a", "qty": 2 },
            { "sku": "b", "qty": 1 },
        ]);

        let rows = Vec::<OrderLine>::from_sql_value(&SqlVal::Json(document))
            .expect("the nested collection decodes");

        assert_eq!(rows, vec![("a".to_owned(), 2), ("b".to_owned(), 1)]);
    }

    #[test]
    fn a_nested_collection_decodes_from_json_text() {
        let rows = Vec::<OrderLine>::from_sql_value(&SqlVal::Text(
            "[{\"sku\":\"a\",\"qty\":2}]".to_owned(),
        ))
        .expect("the JSON text of a nested collection decodes");

        assert_eq!(rows, vec![("a".to_owned(), 2)]);
    }

    #[test]
    fn a_nested_row_round_trips_a_blob_beside_scalars() {
        let row = Document {
            name: "avatar".to_owned(),
            blob: vec![0x00, 0x01, 0x02, 0xfe, 0xff],
        };
        let bound = row.clone().to_sql_val();

        // The binary field carries its canonical hexadecimal text, not the
        // debug rendering of the vector, so the document decodes back into
        // the same row.
        assert_eq!(
            bound,
            SqlVal::Json(serde_json::json!({
                "name": "avatar",
                "blob": "000102feff",
            }))
        );
        assert_eq!(Document::from_sql_value(&bound), Ok(row));
    }

    #[test]
    fn a_nested_blob_field_decodes_the_bytea_text_postgres_reports() {
        let object = json_object(&SqlVal::Json(serde_json::json!({
            "name": "avatar",
            "blob": r"\x000102feff",
        })))
        .expect("the object of the nested row decodes");

        assert_eq!(
            nested_field::<Vec<u8>>(&object, "blob"),
            Ok(vec![0x00, 0x01, 0x02, 0xfe, 0xff])
        );
        assert_eq!(
            nested_field::<String>(&object, "name"),
            Ok("avatar".to_owned())
        );
    }

    #[test]
    fn a_nested_collection_with_blobs_decodes_the_json_text_sqlite_reports() {
        let rows = Vec::<Document>::from_sql_value(&SqlVal::Text(
            r#"[{"name":"avatar","blob":"000102feff"},{"name":"logo","blob":"ff"}]"#
                .to_owned(),
        ))
        .expect("the JSON text of a nested collection decodes");

        assert_eq!(
            rows,
            vec![
                Document {
                    name: "avatar".to_owned(),
                    blob: vec![0x00, 0x01, 0x02, 0xfe, 0xff],
                },
                Document {
                    name: "logo".to_owned(),
                    blob: vec![0xff],
                },
            ]
        );
    }

    #[test]
    fn a_single_nested_row_decodes_into_an_option() {
        let present = Option::<OrderLine>::from_sql_value(&SqlVal::Json(
            serde_json::json!({ "sku": "a", "qty": 2 }),
        ))
        .expect("the nested row decodes");

        assert_eq!(present, Some(("a".to_owned(), 2)));
        assert_eq!(Option::<OrderLine>::from_sql_value(&SqlVal::Null), Ok(None));
        assert_eq!(
            Option::<OrderLine>::from_sql_value(&SqlVal::Text(
                "{\"sku\":\"b\",\"qty\":3}".to_owned()
            )),
            Ok(Some(("b".to_owned(), 3)))
        );
    }

    #[test]
    fn an_integral_json_number_decodes_into_a_float_value() {
        assert_eq!(
            f64::from_sql_value(&json_to_sql_value(&serde_json::json!(2))),
            Ok(2.0)
        );
        assert_eq!(
            f64::from_sql_value(&json_to_sql_value(&serde_json::json!(-1200))),
            Ok(-1200.0)
        );

        let object = json_object(&SqlVal::Json(serde_json::json!({ "total": 1200 })))
            .expect("the object of the nested row decodes");

        assert_eq!(nested_field::<f64>(&object, "total"), Ok(1200.0));
    }

    #[test]
    fn a_nested_collection_decodes_integral_numbers_into_float_fields() {
        // PostgreSQL canonicalises an integral value of a `double precision`
        // column, so the document of a nested row carries `1200` rather than
        // `1200.0`, and exactly that row has to decode into the float field.
        let rows = Vec::<PricedLine>::from_sql_value(&SqlVal::Json(serde_json::json!([
            { "sku": "a", "total": 1200 },
            { "sku": "b", "total": 12.5 },
        ])))
        .expect("the nested collection decodes");

        assert_eq!(
            rows,
            vec![
                PricedLine {
                    sku: "a".to_owned(),
                    total: 1200.0,
                },
                PricedLine {
                    sku: "b".to_owned(),
                    total: 12.5,
                },
            ]
        );
    }

    #[test]
    fn a_nested_collection_decodes_integral_numbers_from_json_text() {
        let rows = Vec::<PricedLine>::from_sql_value(&SqlVal::Text(
            r#"[{"sku":"a","total":1200},{"sku":"b","total":12.5}]"#.to_owned(),
        ))
        .expect("the JSON text of a nested collection decodes");

        assert_eq!(
            rows,
            vec![
                PricedLine {
                    sku: "a".to_owned(),
                    total: 1200.0,
                },
                PricedLine {
                    sku: "b".to_owned(),
                    total: 12.5,
                },
            ]
        );
    }

    #[test]
    fn a_float_collection_decodes_integral_numbers_from_json_text() {
        let values = Vec::<f64>::from_sql_value(&SqlVal::Text("[1200, 12.5]".to_owned()))
            .expect("the JSON text of a float collection decodes");

        assert_eq!(values, vec![1200.0, 12.5]);
    }

    #[test]
    fn an_object_of_another_shape_is_reported() {
        let error = OrderLine::from_sql_value(&SqlVal::Json(serde_json::json!({ "sku": "a" })))
            .expect_err("a missing field is reported");

        assert!(error.to_string().contains("no `qty` field"));
    }

    #[test]
    fn a_scalar_keeps_its_json_form_across_the_helpers() {
        for document in [
            serde_json::Value::Null,
            serde_json::Value::Bool(true),
            serde_json::Value::from(7i64),
            serde_json::Value::from(0.5),
            serde_json::Value::String("root".to_owned()),
        ] {
            let value = json_to_sql_value(&document);

            assert_eq!(sql_val_to_json(&value), document, "round trip of {document}");
        }
    }

    #[test]
    fn a_collection_keeps_the_json_form_of_every_element() {
        let document = serde_json::json!([1, "two", null]);

        let value = json_to_sql_value(&document);

        assert_eq!(sql_val_to_json(&value), document);
        assert_eq!(
            sql_val_to_json(&SqlVal::Array(vec![SqlVal::I64(1), SqlVal::Null])),
            serde_json::json!([1, null])
        );
    }

    #[test]
    fn a_json_document_passes_through_the_helper() {
        let document = serde_json::json!({ "sku": "a", "qty": 2 });

        assert_eq!(sql_val_to_json(&SqlVal::Json(document.clone())), document);
    }

    #[test]
    fn a_value_without_a_json_counterpart_keeps_the_text_it_displays() {
        assert_eq!(
            sql_val_to_json(&SqlVal::EncodingError("not encodable".to_owned())),
            serde_json::json!("not encodable")
        );
    }

    #[test]
    fn a_binary_value_carries_its_canonical_hexadecimal_text() {
        assert_eq!(
            sql_val_to_json(&SqlVal::Bytes(vec![0x00, 0x01, 0x02, 0xfe, 0xff])),
            serde_json::json!("000102feff")
        );
    }

    #[test]
    fn a_row_object_carries_one_key_per_field() {
        let row = multiset_object(&[
            ("sku", SqlVal::Text("a".to_owned())),
            ("qty", SqlVal::I64(2)),
        ]);

        assert_eq!(
            row,
            SqlVal::Json(serde_json::json!({ "sku": "a", "qty": 2 }))
        );

        assert_eq!(OrderLine::from_sql_value(&row), Ok(("a".to_owned(), 2)));
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn an_integer_wider_than_the_signed_range_keeps_every_digit() {
        use rust_decimal::Decimal;

        let document: serde_json::Value = serde_json::from_str(r#"{"id":18446744073709551615}"#)
            .expect("the document is valid JSON");
        let object = json_object(&json_to_sql_value(&document)).expect("the object decodes");

        let id: Decimal = nested_field(&object, "id").expect("the wide number decodes");

        assert_eq!(
            id,
            "18446744073709551615"
                .parse::<Decimal>()
                .expect("the expected value is a decimal")
        );
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn a_fractional_number_carries_the_decimal_kind() {
        use rust_decimal::Decimal;

        assert_eq!(
            json_to_sql_value(&serde_json::json!(12.5)),
            SqlVal::Decimal(Decimal::new(125, 1))
        );
        assert_eq!(
            json_to_sql_value(&serde_json::json!(7)),
            SqlVal::I64(7),
            "an integer keeps the integer representation"
        );
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn a_fractional_number_decodes_into_its_exact_decimal() {
        use rust_decimal::Decimal;

        let document = serde_json::json!({ "total": 12.5 });
        let object = json_object(&json_to_sql_value(&document)).expect("the object decodes");

        let total: Decimal = nested_field(&object, "total").expect("the number decodes");

        assert_eq!(total, Decimal::new(125, 1));
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn an_exact_decimal_round_trips_through_a_nested_field() {
        use rust_decimal::Decimal;

        let exact = "123456789012345678.90123456"
            .parse::<Decimal>()
            .expect("the value fits into a decimal");
        let row = multiset_object(&[("total", SqlVal::Decimal(exact))]);
        let object = json_object(&row).expect("the row decodes");

        let decoded: Decimal = nested_field(&object, "total").expect("the total decodes");

        assert_eq!(decoded, exact);
    }
}

#[cfg(all(test, not(feature = "serde_json")))]
mod without_json_tests {
    use super::*;
    use crate::row::RowDecodeError;

    #[test]
    fn a_nested_row_reports_the_missing_json_feature() {
        let error = match json_object(&SqlVal::Text("{}".to_owned())) {
            Ok(_) => panic!("a build without `serde_json` decodes no nested row"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            RowDecodeError::InvalidValue { detail, .. } if detail.contains("serde_json")
        ));

        assert!(matches!(
            multiset_object(&[("id", SqlVal::I64(1))]),
            SqlVal::EncodingError(_)
        ));
        assert!(nested_field::<i64>(&RowObject, "id").is_err());
    }
}
