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

use crate::dialect::Dialect;
use crate::expr::{render_expr, ExprKind, SqlArrayElement, SqlVal, Subquery};
use crate::query::{HasProjection, SelectQuery, SelectTarget, SelectTargetInScope, TableScope};
use crate::schema::{At, TableRef};

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

    fn render<MD: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) {
        render_expr::<MD>(sql, params, &self.to_expr_kind(), relations);
    }
}

// A nested projection reads from its own statement, so it references no table of
// the enclosing scope and resolves at the first position of every scope. Fixing
// the position keeps the marker determined instead of leaving inference to pick
// one of the positions a scope may offer.
impl<Tables, D, S, Row> SelectTargetInScope<Tables, At<0>>
    for NestMany<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
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

    fn render<MD: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) {
        render_expr::<MD>(sql, params, &self.to_expr_kind(), relations);
    }
}

// A single nested row reads from its own statement as well, so it resolves at the
// same fixed position as the collection form.
impl<Tables, D, S, Row> SelectTargetInScope<Tables, At<0>>
    for NestOne<SelectQuery<D, S, Row, HasProjection>>
where
    D: Dialect,
    S: TableScope,
    Row: MultisetRow,
{
    type Output = Option<Row>;
}

/// Converts a JSON value of a nested document into the matching SQL value.
///
/// Scalars keep their natural representation, so a field of a nested row decodes
/// through exactly the same [`FromSqlValue`](crate::row::FromSqlValue) contract a
/// column of the same type decodes through.
#[cfg(feature = "serde_json")]
pub fn json_to_sql_value(value: &serde_json::Value) -> SqlVal {
    match value {
        serde_json::Value::Null => SqlVal::Null,
        serde_json::Value::Bool(value) => SqlVal::Bool(*value),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(value) => SqlVal::I64(value),
            None => match number.as_f64() {
                Some(value) => SqlVal::F64(value),
                None => SqlVal::Text(number.to_string()),
            },
        },
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
/// A value of a kind a JSON document cannot carry — a binary blob, an exact
/// decimal, a UUID, or a temporal value — keeps the text it is displayed with,
/// which is the scalar form [`FromSqlValue`](crate::row::FromSqlValue) parses back
/// into the same type. A floating point value JSON cannot represent, `NaN` and the
/// two infinities, becomes `null` instead of failing the encoding.
#[cfg(feature = "serde_json")]
pub fn sql_val_to_json(value: &SqlVal) -> serde_json::Value {
    match value {
        SqlVal::Null => serde_json::Value::Null,
        SqlVal::Bool(flag) => serde_json::Value::Bool(*flag),
        SqlVal::I32(number) => serde_json::Value::from(*number),
        SqlVal::I64(number) => serde_json::Value::from(*number),
        SqlVal::F64(number) => json_number(*number),
        SqlVal::Text(text) => serde_json::Value::String(text.clone()),
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
    use crate::expr::Expr;
    use crate::row::{FromSqlValue, RowDecodeError};

    struct PgMock;

    impl Dialect for PgMock {
        const SUPPORTS_JSONB_ROW_AGG: bool = true;

        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

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

    impl crate::expr::ToSqlVal for OrderLine {
        fn to_sql_val(self) -> SqlVal {
            SqlVal::Json(serde_json::json!({ "sku": self.0, "qty": self.1 }))
        }
    }

    impl FromSqlValue for OrderLine {
        fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
            let object = json_object(value)?;
            Ok((
                nested_field(&object, "sku")?,
                nested_field(&object, "qty")?,
            ))
        }
    }

    impl SqlArrayElement for OrderLine {}

    impl MultisetRow for OrderLine {
        const FIELDS: &'static [&'static str] = &["sku", "qty"];
    }

    /// The nested statement of the child rows of every owner.
    fn lines<D: Dialect>() -> SelectQuery<D, Line, OrderLine, HasProjection> {
        SelectQuery::new().from::<Line>().select((LineSku, LineQty))
    }

    #[test]
    fn a_nested_collection_aggregates_into_a_json_array() {
        let built = SelectQuery::<PgMock>::new()
            .from::<Owner>()
            .select((OwnerId, nest_many(lines::<PgMock>())))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", COALESCE((SELECT jsonb_agg(to_jsonb(\"__rurqe_sub\")) FROM \
             (SELECT \"sku\" AS \"sku\", \"qty\" AS \"qty\" FROM \"lines\") AS \"__rurqe_sub\"), \
             '[]'::jsonb) FROM \"owners\""
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn a_single_nested_row_stops_at_its_first_row() {
        let built = SelectQuery::<PgMock>::new()
            .from::<Owner>()
            .select((OwnerId, nest_one(lines::<PgMock>())))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", (SELECT to_jsonb(\"__rurqe_sub\") FROM (SELECT \"sku\" AS \"sku\", \
             \"qty\" AS \"qty\" FROM \"lines\" LIMIT 1) AS \"__rurqe_sub\") FROM \"owners\""
        );
    }

    #[test]
    fn a_dialect_without_native_row_aggregation_assembles_the_object() {
        let built = SelectQuery::<SqliteMock>::new()
            .from::<Owner>()
            .select((OwnerId, nest_many(lines::<SqliteMock>())))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", COALESCE((SELECT json_group_array(json_object('sku', \
             \"__rurqe_sub\".\"sku\", 'qty', \"__rurqe_sub\".\"qty\")) FROM (SELECT \"sku\" AS \
             \"sku\", \"qty\" AS \"qty\" FROM \"lines\") AS \"__rurqe_sub\"), '[]') FROM \"owners\""
        );
    }

    #[test]
    fn a_single_row_of_a_dialect_without_native_aggregation_is_an_object() {
        let built = SelectQuery::<SqliteMock>::new()
            .from::<Owner>()
            .select((OwnerId, nest_one(lines::<SqliteMock>())))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", (SELECT json_object('sku', \"__rurqe_sub\".\"sku\", 'qty', \
             \"__rurqe_sub\".\"qty\") FROM (SELECT \"sku\" AS \"sku\", \"qty\" AS \"qty\" FROM \
             \"lines\" LIMIT 1) AS \"__rurqe_sub\") FROM \"owners\""
        );
    }

    #[test]
    fn the_nested_statement_binds_its_parameters_in_place() {
        let inner = SelectQuery::<PgMock>::new()
            .from::<Line>()
            .select((LineSku, LineQty))
            .where_(Expr::column::<LineQty>().gt(1i64));
        let built = SelectQuery::<PgMock>::new()
            .from::<Owner>()
            .select((OwnerId, nest_many(inner)))
            .build();

        assert_eq!(
            built.sql,
            "SELECT \"id\", COALESCE((SELECT jsonb_agg(to_jsonb(\"__rurqe_sub\")) FROM \
             (SELECT \"sku\" AS \"sku\", \"qty\" AS \"qty\" FROM \"lines\" WHERE (\"qty\" > $1)) \
             AS \"__rurqe_sub\"), '[]'::jsonb) FROM \"owners\""
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
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
            sql_val_to_json(&SqlVal::Bytes(vec![1, 2])),
            serde_json::json!("[1, 2]")
        );
        assert_eq!(
            sql_val_to_json(&SqlVal::EncodingError("not encodable".to_owned())),
            serde_json::json!("not encodable")
        );
    }

    #[test]
    fn a_row_object_built_from_sql_values_decodes_back() {
        let document = serde_json::json!({
            "sku": sql_val_to_json(&SqlVal::Text("a".to_owned())),
            "qty": sql_val_to_json(&SqlVal::I64(2)),
        });

        assert_eq!(
            OrderLine::from_sql_value(&SqlVal::Json(document)),
            Ok(("a".to_owned(), 2))
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
