//! End-to-end verification of the `dto!` reusable projection macro.
//!
//! The tests exercise the struct the macro declares: schema-driven field typing,
//! marker-derived and aliased field names, row decoding, and rejection of rows
//! whose arity or value types do not match the declaration. Fields are accessed
//! by name only, never through tuple indices.

use rurqe_core::expr::SqlVal;
use rurqe_core::row::FromRow;
use rurqe_macros::dto;

rurqe_core::table! {
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
        UserActive => "active": bool,
    }
}

rurqe_core::table! {
    Keyword => "keywords" {
        Type => "type": i64,
    }
}

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
