//! End-to-end verification of the universal `select!` named projection macro.
//!
//! The tests exercise the anonymous record the macro synthesises: ordered column
//! projection, schema-driven field typing, alias renaming, computed expression
//! projections, automatic nullability lifting of outer joins, row decoding, and
//! integration with the query builder. Rows are inspected through named fields
//! only, never through tuple indices.

use rurqe_core::dialect::Dialect;
use rurqe_core::expr::{Expr, SqlVal};
use rurqe_core::query::{LeftJoined, SelectQuery, Selection, SelectionFor};
use rurqe_core::row::FromRow;
use rurqe_macros::select;

rurqe_core::table! {
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
        UserActive => "active": bool,
    }
}

rurqe_core::table! {
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
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

/// Decodes `values` into the row type `S` produces in the relational scope `Tables`.
fn decode_scoped<S, Tables, Positions>(
    _selection: &S,
    values: &[SqlVal],
) -> Option<<S as SelectionFor<Tables, Positions>>::Row>
where
    S: SelectionFor<Tables, Positions>,
    <S as SelectionFor<Tables, Positions>>::Row: FromRow,
{
    <S as SelectionFor<Tables, Positions>>::Row::from_row(values)
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
fn named_expressions_project_computed_fields() {
    let selection = select!(
        user_id: UserId,
        doubled: Expr::<i64>::raw("id * 2", vec![]),
    );

    let row = decode(&selection, &[SqlVal::I64(2), SqlVal::I64(4)]).expect("row decodes");

    assert_eq!(row.user_id, 2);
    assert_eq!(row.doubled, 4);
}

#[test]
fn expressions_render_through_the_select_target_trait() {
    let built = SelectQuery::<TestDialect>::new()
        .from::<User>()
        .select(select!(UserId, Expr::sum::<UserId>() as total))
        .build();

    assert_eq!(built.sql, "SELECT users.id, SUM(id) FROM users");
    assert!(built.params.is_empty());
}

#[test]
fn selection_starts_a_query_directly_from_a_table() {
    let built = select!(UserId, UserEmail as contact_email)
        .from::<TestDialect, User>()
        .where_(Expr::column::<UserId>().gt(1i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT users.id, users.email FROM users WHERE (id > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn left_joined_columns_are_lifted_into_options() {
    let selection = select!(UserId, OrderTotal);

    let row = decode_scoped::<_, LeftJoined<User, Order>, _>(
        &selection,
        &[SqlVal::I64(1), SqlVal::Null],
    )
    .expect("row decodes");

    assert_eq!(row.user_id, 1);
    assert_eq!(row.order_total, None);
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

    assert_eq!(built.sql, "SELECT users.id, users.email FROM users");
}
