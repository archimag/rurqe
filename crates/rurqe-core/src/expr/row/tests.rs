use super::*;

use crate::dialect::Dialect;
use crate::expr::{render_expr, SqlVal};
use crate::query::SelectQuery;
use crate::schema::TableRef;

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

crate::table! {
    Order => "orders" {
        OrderId => "id": i64,
        OrderName => "name": String,
    }
}

struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&index.to_string());
    }
}

fn render_with(kind: &ExprKind, relations: &[TableRef]) -> (String, Vec<SqlVal>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<MockDialect>(&mut sql, &mut params, kind, relations);
    (sql, params)
}

fn render(kind: &ExprKind) -> (String, Vec<SqlVal>) {
    render_with(kind, &[])
}

#[test]
fn a_row_value_comparison_renders_a_native_tuple_comparison() {
    let predicate = (UserId, UserName).gt((10i64, "root"));

    let (sql, params) = render(predicate.kind());

    assert_eq!(sql, "((\"id\", \"name\") > ($1, $2))");
    assert_eq!(
        params,
        vec![SqlVal::I64(10), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn every_ordering_operator_renders_its_sql_text() {
    assert_eq!(
        render((UserId, UserName).eq((1i64, "a")).kind()).0,
        "((\"id\", \"name\") = ($1, $2))"
    );
    assert_eq!(
        render((UserId, UserName).ne((1i64, "a")).kind()).0,
        "((\"id\", \"name\") <> ($1, $2))"
    );
    assert_eq!(
        render((UserId, UserName).gt((1i64, "a")).kind()).0,
        "((\"id\", \"name\") > ($1, $2))"
    );
    assert_eq!(
        render((UserId, UserName).lt((1i64, "a")).kind()).0,
        "((\"id\", \"name\") < ($1, $2))"
    );
    assert_eq!(
        render((UserId, UserName).gte((1i64, "a")).kind()).0,
        "((\"id\", \"name\") >= ($1, $2))"
    );
    assert_eq!(
        render((UserId, UserName).lte((1i64, "a")).kind()).0,
        "((\"id\", \"name\") <= ($1, $2))"
    );
    assert_eq!(BinaryOp::Gte.as_str(), ">=");
    assert_eq!(BinaryOp::Lte.as_str(), "<=");
}

#[test]
fn a_row_comparison_records_both_sides_as_row_nodes() {
    let predicate = (UserId, UserName).eq((1i64, "root"));

    let ExprKind::Binary { op, lhs, rhs } = predicate.kind() else {
        panic!("expected a binary comparison");
    };

    assert_eq!(*op, BinaryOp::Eq);
    assert!(matches!(**lhs, ExprKind::Row { .. }));
    assert!(matches!(**rhs, ExprKind::Row { .. }));
}

#[test]
fn rows_of_up_to_eight_elements_compare_as_one_value() {
    let three = (UserId, UserName, UserId).eq((1i64, "a", 2i64));
    assert_eq!(
        render(three.kind()).0,
        "((\"id\", \"name\", \"id\") = ($1, $2, $3))"
    );

    let eight = (
        UserId, UserName, UserId, UserName, UserId, UserName, UserId, UserName,
    )
        .eq((1i64, "a", 2i64, "b", 3i64, "c", 4i64, "d"));
    assert_eq!(
        render(eight.kind()).0,
        "((\"id\", \"name\", \"id\", \"name\", \"id\", \"name\", \"id\", \"name\") = ($1, $2, $3, $4, $5, $6, $7, $8))"
    );
}

#[test]
fn the_row_scope_merges_the_tables_of_its_elements() {
    let same_table: Expr<bool, (User, User)> = (UserId, UserName).eq((1i64, "root"));
    assert!(matches!(same_table.kind(), ExprKind::Binary { .. }));

    let across_tables: Expr<bool, (User, Order)> = (UserId, OrderId).eq((1i64, 2i64));
    assert!(matches!(across_tables.kind(), ExprKind::Binary { .. }));
}

#[test]
fn row_values_accept_built_expressions() {
    let predicate = (UserId, UserName).eq((Expr::column::<OrderId>(), Expr::column::<OrderName>()));

    let (sql, params) = render_with(
        predicate.kind(),
        &[TableRef::plain("users"), TableRef::plain("orders")],
    );

    assert_eq!(
        sql,
        "((\"users\".\"id\", \"users\".\"name\") = (\"orders\".\"id\", \"orders\".\"name\"))"
    );
    assert!(params.is_empty());
}

#[test]
fn a_row_matches_the_rows_of_a_subquery() {
    let subquery = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, OrderName));

    let predicate = (UserId, UserName).in_subquery(subquery);

    let (sql, params) = render(predicate.kind());

    assert_eq!(
        sql,
        "((\"id\", \"name\") IN (SELECT \"id\", \"name\" FROM \"orders\"))"
    );
    assert!(params.is_empty());
}

#[test]
fn a_row_membership_predicate_can_be_negated() {
    let subquery = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, OrderName));

    let predicate = (UserId, UserName).not_in_subquery(subquery);

    let (sql, _) = render(predicate.kind());

    assert_eq!(
        sql,
        "((\"id\", \"name\") NOT IN (SELECT \"id\", \"name\" FROM \"orders\"))"
    );
}

#[test]
fn a_row_predicate_filters_a_query() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_((UserId, UserName).gt((10i64, "root")));

    assert_eq!(
        query.build().sql,
        "SELECT \"id\" FROM \"users\" WHERE ((\"id\", \"name\") > ($1, $2))"
    );
}
