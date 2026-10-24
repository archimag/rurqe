//! Regression tests for the projection rule of a grouped query.
//!
//! A grouped query accepts a projection without an aggregate only when the
//! projection is genuinely constant, which the empty scope `()` states. The tests
//! pin that `coalesce`, `CASE`, and `Expr::without_scope` keep the tables their
//! expressions read instead of degrading into that empty scope.

use crate::dialect::Dialect;
use crate::expr::{coalesce, Aggregate, Expr, SqlVal, Unscoped};
use crate::{row_number, SelectQuery, Window};

struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        use core::fmt::Write;
        let _ = write!(out, "${index}");
    }
}

impl crate::dialect::SupportsWindowFunctions for MockDialect {}

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

crate::table! {
    Order => "orders" {
        OrderUserId => "user_id": i64,
        OrderTotal => "total": f64,
    }
}

#[test]
fn coalesce_keeps_the_scope_of_its_column() {
    let _: Expr<String, User> = coalesce(UserName, "anonymous");
    let _: Expr<f64, Order> = coalesce(OrderTotal, 0.0);

    // A fallback reading a column merges its own scope as well.
    let _: Expr<f64, (Order, Order)> = coalesce(OrderTotal, Expr::column::<OrderTotal>());
}

#[test]
fn a_case_expression_keeps_the_scope_of_its_branches() {
    let _: Expr<String, Order> = Expr::<String>::case()
        .when(Expr::column::<OrderTotal>().gt(10.0), "large")
        .else_("small")
        .end();

    // A condition and a result of different tables merge both scopes.
    let _: Expr<f64, (User, Order)> = Expr::<f64>::case()
        .when(
            Expr::column::<UserId>().gt(1i64),
            Expr::column::<OrderTotal>(),
        )
        .else_(0.0)
        .end();

    // The fallback contributes the scope of its own table.
    let _: Expr<f64, (User, Order)> = Expr::<f64>::case()
        .when(Expr::column::<UserId>().gt(1i64), 0.0)
        .else_(Expr::column::<OrderTotal>())
        .end();
}

#[test]
fn dropping_the_scope_marks_the_expression_unscoped() {
    let _: Expr<i64, Unscoped> = Expr::column::<UserId>().without_scope();

    // The marker adds nothing to the scope of the other operand, so a correlated
    // predicate stays checked against the tables of the statement it belongs to.
    let _: Expr<bool, User> = Expr::column::<UserId>().eq(Expr::column::<UserId>().without_scope());

    // A predicate built over an unscoped column keeps the marker: it is accepted
    // in every scope, yet it never counts as a constant.
    let _: Expr<bool, Unscoped> = Expr::column::<UserId>().without_scope().eq(1i64);
}

#[test]
fn a_grouped_query_accepts_a_constant_projection() {
    let counted: Expr<i64> =
        Expr::<i64>::raw("COUNT(*)", Vec::new()).expect("a template without placeholders");

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, counted))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\""
    );
}

#[test]
fn aggregate_comparisons_keep_the_aggregation_marker() {
    // An aggregate collapses the rows of a group into one value, so a comparison
    // written over it stays a valid HAVING predicate instead of degrading into a
    // comparison evaluated per row.
    let _: Expr<bool, (), Aggregate> = Expr::count_all().gt(5i64);
    let _: Expr<bool, Order, Aggregate> = Expr::sum::<OrderTotal>().lt(100.0);

    // A bare column comparison is evaluated per row, so it keeps the table of its
    // column and carries no aggregation marker.
    let _: Expr<bool, Order> = Expr::column::<OrderTotal>().gt(10.0);
}

#[test]
fn a_grouped_query_accepts_aggregates_over_ungrouped_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all(), Expr::sum::<OrderTotal>()))
        .having(Expr::count_all().gt(1i64))
        .order_by_expr_desc(Expr::sum::<OrderTotal>())
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*), SUM(\"total\") FROM \"orders\" GROUP BY \"user_id\" \
         HAVING (COUNT(*) > $1) ORDER BY SUM(\"total\") DESC"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_grouped_query_accepts_a_window_over_its_grouped_columns() {
    // A window function is evaluated once the rows are grouped, so a
    // specification partitioning and ordering by grouped columns stays
    // projectable from the grouped query.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by((OrderUserId, OrderTotal))
        .select((
            OrderUserId,
            row_number().over(Window::partition_by(OrderUserId).order_by_asc(OrderTotal)),
        ))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", ROW_NUMBER() OVER (PARTITION BY \"user_id\" ORDER BY \"total\" ASC) \
         FROM \"orders\" GROUP BY \"user_id\", \"total\""
    );
    assert!(built.params.is_empty());
}
