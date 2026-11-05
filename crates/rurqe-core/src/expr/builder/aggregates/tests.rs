//! Rendering tests of the aggregate clauses and of the guards behind them.

use super::{DirectAggregate, Expr};
use crate::dialect::{Dialect, SupportsAggregateFilter};
use crate::expr::{count_distinct, render_expr, sum, Aggregate, ExprKind, SqlVal};
use crate::query::SelectQuery;
use crate::schema::TableRef;

crate::table!(
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderNote => "note": String,
    }
);

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

// The mock dialect stands for a modern engine, so filtered aggregates render.
impl SupportsAggregateFilter for MockDialect {}

fn render(expr: &ExprKind) -> (String, Vec<SqlVal>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<MockDialect>(&mut sql, &mut params, expr, &[TableRef::plain("orders")])
        .expect("valid expression");
    (sql, params)
}

#[test]
fn a_distinct_modifier_renders_inside_the_call() {
    let (sql, params) = render(count_distinct(OrderId).kind());
    assert_eq!(sql, "COUNT(DISTINCT \"id\")");
    assert!(params.is_empty());

    let (sql, params) = render(sum(OrderTotal).distinct().kind());
    assert_eq!(sql, "SUM(DISTINCT \"total\")");
    assert!(params.is_empty());
}

#[test]
fn a_filter_clause_renders_after_the_call_and_binds_its_predicate() {
    let (sql, params) = render(
        Expr::<i64>::count::<OrderId>()
            .filter_where(Expr::column::<OrderId>().gt(10i64))
            .kind(),
    );

    assert_eq!(sql, "COUNT(\"id\") FILTER (WHERE (\"id\" > $1))");
    assert_eq!(params, vec![SqlVal::I64(10)]);
}

#[test]
fn repeated_filter_clauses_accumulate_on_the_call() {
    let (sql, params) = render(
        Expr::<i64>::count::<OrderId>()
            .filter_where(Expr::column::<OrderId>().gt(10i64))
            .filter_where(Expr::column::<OrderId>().lt(50i64))
            .kind(),
    );

    assert_eq!(
        sql,
        "COUNT(\"id\") FILTER (WHERE ((\"id\" > $1) AND (\"id\" < $2)))"
    );
    assert_eq!(params, vec![SqlVal::I64(10), SqlVal::I64(50)]);
}

#[test]
fn a_forged_marker_keeps_the_dropped_modifier_visible() {
    // A hand-assembled tree: the marker names an aggregate call while the
    // root is a bound value. The public API cannot reach this state, so the
    // guard is the total defence that keeps the modifier from vanishing
    // silently should such a tree ever appear.
    let forged: Expr<i64, (), DirectAggregate> = Expr::new(ExprKind::Value(SqlVal::I64(1)));

    let marked = forged.distinct();

    let ExprKind::Raw { fragments, params } = marked.kind() else {
        panic!(
            "expected the diagnostic placeholder, got {:?}",
            marked.kind()
        );
    };
    assert_eq!(
        fragments.first(),
        Some("/* invalid aggregate marker: DISTINCT modifier dropped */")
    );
    assert!(params.is_empty());
}

#[test]
fn a_forged_marker_keeps_the_dropped_filter_visible() {
    // A hand-assembled tree: the marker names an aggregate call while the
    // root is a bare column reference. The guard reports the discarded
    // predicate in the statement instead of hiding it.
    let forged: Expr<i64, Order, DirectAggregate> = Expr::new(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    let marked = forged.filter_where(Expr::column::<OrderId>().gt(10i64));

    let ExprKind::Raw { fragments, params } = marked.kind() else {
        panic!(
            "expected the diagnostic placeholder, got {:?}",
            marked.kind()
        );
    };
    assert_eq!(
        fragments.first(),
        Some("/* invalid aggregate marker: FILTER predicate dropped */")
    );
    assert!(params.is_empty());
}

#[test]
fn a_new_root_degrades_the_aggregate_call_to_the_general_marker() {
    // `DISTINCT` and `FILTER (WHERE ...)` belong to the aggregate call itself,
    // so a builder that roots a new node over the call degrades the marker the
    // same way an arithmetic combination does: the produced expression still
    // collapses the rows of its group and stays projectable from a grouped
    // query, while neither clause stays reachable on it.
    fn assert_general<T, Scope, D>(_: Expr<T, Scope, Aggregate, D>) {}

    assert_general(Expr::<i64>::count::<OrderId>().is_null());
    assert_general(Expr::<i64>::count::<OrderId>().is_not_null());
    assert_general(Expr::<i64>::sum::<OrderId>().cast::<i32>());
    assert_general(
        sum(OrderId)
            .filter_where(Expr::column::<OrderId>().gt(0i64))
            .is_null(),
    );
    assert_general(Expr::<String>::max::<OrderNote>().like("A%"));
    assert_general(Expr::<String>::max::<OrderNote>().ilike("a%"));
    assert_general(Expr::<i64>::sum::<OrderId>().dialect_op::<i64, i64, _>("+", 1i64));
    assert_general(Expr::<i64>::sum::<OrderId>().dialect_quantified::<i64, _>("ANY", 1i64));

    let nested = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,));
    assert_general(Expr::<i64>::sum::<OrderId>().in_subquery(nested));

    let nested = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,));
    assert_general(Expr::<i64>::sum::<OrderId>().not_in_subquery(nested));
}

#[test]
fn a_new_root_places_its_own_node_at_the_root() {
    // The rendered statement carries the node the builder created, so the
    // aggregate call sits inside it instead of at the root.
    let predicate = Expr::<i64>::sum::<OrderId>().is_null();
    assert!(matches!(
        predicate.kind(),
        ExprKind::IsNull { negated: false, .. }
    ));

    let cast = sum(OrderId).cast::<i32>();
    let (sql, params) = render(cast.kind());
    assert_eq!(sql, "CAST(SUM(\"id\") AS INTEGER)");
    assert!(params.is_empty());
}
