//! End-to-end verification of the `dto!` reusable projection macro.
//!
//! The tests exercise the struct the macro declares: schema-driven field typing,
//! marker-derived and aliased field names, row decoding, and rejection of rows
//! whose arity or value types do not match the declaration. Fields are accessed
//! by name only, never through tuple indices. The last group drives the same
//! declaration through the nested-row contracts, where a DTO is read from and
//! written back into the JSON document of one related row.

use rurqe_core::expr::{SqlArrayElement, SqlVal, ToSqlVal};
use rurqe_core::nested::MultisetRow;
use rurqe_core::row::{FromRow, FromSqlValue};
use rurqe_macros::dto;

rurqe_core::table!(
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
        UserActive => "active": bool,
    }
);

rurqe_core::table!(
    Keyword => "keywords" {
        Type => "type": i64,
    }
);

dto! {
    /// A user projection returned by repository functions.
    pub struct UserSummary {
        UserId,
        UserEmail as contact_email,
    }
}

dto! {
    pub struct KeywordDto {
        Type,
    }
}

dto! {
    pub(crate) struct UserFlags {
        UserActive,
    }
}

#[test]
fn fields_are_named_after_columns_and_aliases() {
    let summary = UserSummary {
        user_id: 1,
        contact_email: "root".to_owned(),
    };

    assert_eq!(summary.user_id, 1);
    assert_eq!(summary.contact_email, "root");
}

#[test]
fn field_types_are_read_from_the_schema() {
    let summary = UserSummary::from_row(&[SqlVal::I64(1), SqlVal::Text("root".to_owned())])
        .expect("row decodes");

    let user_id: i64 = summary.user_id;
    let contact_email: String = summary.contact_email.clone();

    assert_eq!(user_id, 1);
    assert_eq!(contact_email, "root");
    assert_eq!(
        summary,
        UserSummary {
            user_id: 1,
            contact_email: "root".to_owned(),
        }
    );
}

#[test]
fn single_field_dto_decodes_rows() {
    let flags = UserFlags::from_row(&[SqlVal::Bool(true)]).expect("row decodes");

    assert!(flags.user_active);
}

#[test]
fn rejects_arity_and_type_mismatch() {
    assert!(UserSummary::from_row(&[SqlVal::I64(1)]).is_none());
    assert!(UserSummary::from_row(&[
        SqlVal::I64(1),
        SqlVal::Text("root".to_owned()),
        SqlVal::Bool(true),
    ])
    .is_none());
    assert!(UserSummary::from_row(&[
        SqlVal::Text("1".to_owned()),
        SqlVal::Text("root".to_owned()),
    ])
    .is_none());
}

#[test]
fn reserved_keywords_become_raw_identifiers() {
    let record = KeywordDto::from_row(&[SqlVal::I64(3)]).expect("row decodes");

    assert_eq!(record.r#type, 3);
}

#[test]
fn the_nested_keys_follow_the_declared_field_names() {
    let summary: &[&str] = &["user_id", "contact_email"];
    let keyword: &[&str] = &["type"];

    assert_eq!(UserSummary::FIELDS, summary);
    assert_eq!(KeywordDto::FIELDS, keyword);
}

#[test]
fn a_dto_writes_the_object_of_a_nested_row() {
    let encoded = UserSummary {
        user_id: 1,
        contact_email: "root".to_owned(),
    }
    .to_sql_val();

    assert_eq!(
        encoded,
        SqlVal::Json(serde_json::json!({ "user_id": 1, "contact_email": "root" }))
    );
}

#[test]
fn a_dto_reads_the_object_of_a_nested_row() {
    let document = SqlVal::Json(serde_json::json!({ "user_id": 1, "contact_email": "root" }));

    let summary = UserSummary::from_sql_value(&document).expect("the nested row decodes");

    assert_eq!(
        summary,
        UserSummary {
            user_id: 1,
            contact_email: "root".to_owned(),
        }
    );
}

#[test]
fn a_dto_reads_the_json_text_of_a_nested_row() {
    let document = SqlVal::Text("{\"user_id\":2,\"contact_email\":\"ops\"}".to_owned());

    let summary = UserSummary::from_sql_value(&document).expect("the nested row decodes");

    assert_eq!(summary.user_id, 2);
    assert_eq!(summary.contact_email, "ops");
}

#[test]
fn a_missing_field_of_a_nested_row_is_reported() {
    let document = SqlVal::Json(serde_json::json!({ "user_id": 1 }));

    let error = UserSummary::from_sql_value(&document).expect_err("the missing field is reported");

    assert!(error.to_string().contains("contact_email"));
}

#[test]
fn a_collection_of_dtos_decodes_from_a_json_array() {
    let document = SqlVal::Json(serde_json::json!([
        { "user_id": 1, "contact_email": "root" },
        { "user_id": 2, "contact_email": "ops" },
    ]));

    let rows = Vec::<UserSummary>::from_sql_value(&document).expect("the collection decodes");

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.get(1).map(|row| row.contact_email.as_str()),
        Some("ops")
    );
}

#[test]
fn a_dto_is_an_array_element() {
    fn assert_element<T: SqlArrayElement>() {}

    assert_element::<UserSummary>();
    assert_element::<KeywordDto>();
    assert_element::<UserFlags>();
}
