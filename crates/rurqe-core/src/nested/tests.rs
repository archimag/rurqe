//! Rendering and decoding tests of the nested projections.

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
