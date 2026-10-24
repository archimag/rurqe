//! Nested statements used as predicates.

use super::{MockDialect, Order, OrderId, OrderTotal, OrderUserId, User, UserId};
use crate::expr::{Expr, SqlVal};
use crate::query::*;

#[test]
fn subquery_predicates_render_nested_queries_with_sequential_parameters() {
    let large_orders = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId,))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let recent_orders = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().lt(10.0));

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().in_subquery(large_orders))
        .where_(Expr::not_exists(recent_orders))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"users\" WHERE ((\"id\" IN (SELECT \"user_id\" FROM \"orders\" \
         WHERE (\"total\" > $1))) AND (NOT EXISTS (SELECT \"id\" FROM \"orders\" WHERE \
         (\"total\" < $2))))"
    );
    assert_eq!(built.params, vec![SqlVal::F64(100.0), SqlVal::F64(10.0)]);
}

#[test]
fn a_correlated_reference_names_its_table_inside_a_single_relation_subquery() {
    // The inner statement addresses `orders` alone, so a bare reference would read
    // as a column of that statement and silently compare the outer row with
    // itself. Dropping the scope makes the reference name its own table, so the
    // correlated reading stays explicit even when both statements scan one table.
    let earlier = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderId>().lt(Expr::column::<OrderId>().without_scope()))
        .order_by_asc(OrderId)
        .limit(1);

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, OrderTotal))
        .where_(Expr::exists(earlier))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"total\" FROM \"orders\" WHERE (EXISTS (SELECT \"id\" FROM \"orders\" \
         WHERE (\"id\" < \"orders\".\"id\") ORDER BY \"id\" ASC LIMIT 1))"
    );
    assert!(built.params.is_empty());
}

#[test]
fn not_in_subquery_renders_negated_membership() {
    let totals = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,));

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().not_in_subquery(totals))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE (\"total\" NOT IN (SELECT \"total\" FROM \"orders\"))"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_distinct_on_prefix_violation_inside_a_subquery_is_caught_by_the_outer_build() {
    let inner = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .distinct_on(UserId)
        .order_by_asc(super::UserName);

    let outer = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderUserId>().in_subquery(inner));

    assert_eq!(
        outer.build(),
        Err(BuildError::DistinctOnOrderMismatch { position: 0 })
    );
}
