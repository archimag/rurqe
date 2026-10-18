//! End-to-end verification of the `select!` ad-hoc projection macro.
//!
//! The tests exercise the anonymous record the macro synthesises: ordered
//! column projection, schema-driven field typing, alias renaming, row decoding,
//! and integration with the query builder. Rows are inspected through named
//! fields only, never through tuple indices.

use rurqe_core::dialect::Dialect;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{SelectQuery, Selection};
use rurqe_core::row::FromRow;
use rurqe_macros::select;

rurqe_core::table! {
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
        UserActive => "active": bool,
    }
}

/// A minimal dialect that renders bare identifiers and `$N` placeholders.
struct TestDialect;

impl Dialect for TestDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push_str(ident);
    }

    fn push_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&index.to_string());
    }
}

/// Decodes `values` into the row type of the anonymous selection `S`.
fn decode<S>(_selection: &S, values: &[SqlVal]) -> Option<S::Row>
where
    S: Selection,
    S::Row: FromRow,
{
    S::Row::from_row(values)
}

#[test]
fn projects_columns_in_declaration_order() {
    let selection = select!(UserId, UserEmail, UserActive);

    let mut names = Vec::new();
    selection.push_column_names(&mut names);

    assert_eq!(names, vec!["id", "email", "active"]);
}

#[test]
fn derives_field_names_from_column_markers() {
    let selection = select!(UserId, UserEmail);

    let row = decode(
        &selection,
        &[SqlVal::I64(1), SqlVal::Text("root".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(row.user_id, 1);
    assert_eq!(row.user_email, "root");
}

#[test]
fn aliases_rename_named_fields() {
    let selection = select!(UserId as id, UserEmail as contact_email);

    let row = decode(
        &selection,
        &[SqlVal::I64(9), SqlVal::Text("x@y.z".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(row.id, 9);
    assert_eq!(row.contact_email, "x@y.z");
}

#[test]
fn rejects_arity_and_type_mismatch() {
    let selection = select!(UserId);

    assert!(decode(&selection, &[]).is_none());
    assert!(decode(&selection, &[SqlVal::I64(1), SqlVal::I64(2)]).is_none());
    assert!(decode(&selection, &[SqlVal::Text("1".to_owned())]).is_none());
}

#[test]
fn selection_feeds_the_query_builder() {
    let built = SelectQuery::<TestDialect>::new()
        .from::<User>()
        .select(select!(UserId, UserEmail as contact_email))
        .build();

    assert_eq!(built.sql, "SELECT id, email FROM users");
}
