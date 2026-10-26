//! Regression tests for the projection rule of a grouped query.
//!
//! A grouped query accepts a projection without an aggregate only when the
//! projection is genuinely constant, which the empty scope `()` states. The tests
//! pin that `coalesce`, `CASE`, and `Expr::without_scope` keep the tables their
//! expressions read instead of degrading into that empty scope.

//! One test pins that the builder drains a borrowed selection in place, so the
//! contract of `GroupedQuery::select` stays the two semantic traits it shares
//! with `SelectQuery::select` instead of demanding an owned, thread-safe one.

use crate::dialect::Dialect;
use crate::expr::{coalesce, Aggregate, Expr, ExprKind, SqlVal, Unscoped};
use crate::query::{GroupedSelection, Selection, SelectionFor};
use crate::schema::At;
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
fn a_scalar_function_over_an_aggregate_stays_an_aggregate() {
    // An aggregate argument keeps the whole call an aggregate, so the call is
    // projectable from a grouped query even though it reads an ungrouped column.
    let _: Expr<f64, Order, Aggregate> = coalesce(Expr::sum::<OrderTotal>(), 0.0);

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, coalesce(Expr::sum::<OrderTotal>(), 0.0)))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COALESCE(SUM(\"total\"), $1) FROM \"orders\" GROUP BY \"user_id\""
    );
    assert_eq!(built.params, vec![SqlVal::F64(0.0)]);
}

#[test]
fn a_grouped_query_accepts_a_case_over_aggregates() {
    // Every condition and result of the CASE is an aggregate or a constant, so
    // the expression collapses the rows of a group and the grouped query
    // accepts it as a projection.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((
            OrderUserId,
            Expr::<f64>::case()
                .when(Expr::sum::<OrderTotal>().gt(0.0), Expr::sum::<OrderTotal>())
                .else_(0.0)
                .end(),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", CASE WHEN (SUM(\"total\") > $1) THEN SUM(\"total\") ELSE $2 END \
         FROM \"orders\" GROUP BY \"user_id\""
    );
    assert_eq!(built.params, vec![SqlVal::F64(0.0), SqlVal::F64(0.0)]);
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
        .build()
        .expect("valid query");

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
        .build()
        .expect("valid query");

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
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", ROW_NUMBER() OVER (PARTITION BY \"user_id\" ORDER BY \"total\" ASC) \
         FROM \"orders\" GROUP BY \"user_id\", \"total\""
    );
    assert!(built.params.is_empty());
}

/// A projection borrowing the SQL text it renders.
///
/// `GroupedQuery::select` drains the projection into the statement at the call
/// site and keeps nothing of the selection, so a selection borrowing its state is
/// consumed exactly like an owned one. The type states that directly instead of
/// going through a tuple: the builder records one item, so it answers the
/// single-item selection contracts itself.
struct Borrowed<'a> {
    /// The SQL text of the projection, borrowed for the life of the query.
    sql: &'a str,
}

impl Selection for Borrowed<'_> {
    type Row = (i64,);

    fn push_projections(&self, out: &mut Vec<ExprKind>) {
        let projection =
            Expr::<i64>::raw(self.sql, Vec::new()).expect("a template without placeholders");
        out.push(projection.into_kind());
    }
}

impl<Tables, D> SelectionFor<Tables, At<0>, D> for Borrowed<'_> {
    type Row = (i64,);
}

impl<GroupByCols, D> GroupedSelection<GroupByCols, At<0>, D> for Borrowed<'_> {}

#[test]
fn a_grouped_query_projects_a_borrowed_selection() {
    // The selection borrows the text it renders, so it is neither owned nor
    // `'static`: the builder drains it in place, exactly like an owned one.
    let label = String::from("COUNT(*)");

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select(Borrowed { sql: &label })
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT COUNT(*) FROM \"orders\" GROUP BY \"user_id\""
    );
    assert!(built.params.is_empty());
}
