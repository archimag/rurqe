//! Window functions and their frames.

use super::{
    MockDialect, Order, OrderAlias, OrderId, OrderTotal, OrderUserId, User, UserId, UserName,
};
use crate::expr::Expr;
use crate::query::*;
use crate::schema::AliasedCol;

#[test]
fn window_functions_render_expected_over_clauses() {
    let window = Window::partition_by(UserId).order_by_asc(UserName);

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((
            UserId,
            row_number().over(window.clone()),
            rank().over(window.clone()),
            dense_rank().over(window.clone()),
            lag(UserName).offset(2).over(Window::order_by_desc(UserId)),
            lead(UserName).over(Window::empty()),
            Expr::count_all().over(Window::partition_by(UserId)),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", ROW_NUMBER() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
         RANK() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
         DENSE_RANK() OVER (PARTITION BY \"id\" ORDER BY \"name\" ASC), \
         LAG(\"name\", 2) OVER (ORDER BY \"id\" DESC), \
         LEAD(\"name\") OVER (), \
         COUNT(*) OVER (PARTITION BY \"id\") \
         FROM \"users\""
    );
}

#[test]
fn grouped_selection_accepts_window_expressions() {
    // A window is evaluated over the rows of a group, so it reads only the columns
    // the query groups by: partitioning by the grouped user and ordering by the
    // grouped identifier stays accepted, while a window reading an ungrouped column
    // is rejected at the call site.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by((OrderUserId, OrderId))
        .select((
            OrderUserId,
            row_number().over(Window::partition_by(OrderUserId).order_by_asc(OrderId)),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", ROW_NUMBER() OVER (PARTITION BY \"user_id\" ORDER BY \"id\" ASC) \
         FROM \"orders\" GROUP BY \"user_id\", \"id\""
    );
}

#[test]
fn window_specifications_and_lag_qualify_aliased_columns() {
    let window = Window::partition_by(AliasedCol::<OrderAlias, OrderUserId>::new())
        .order_by_desc(AliasedCol::<OrderAlias, OrderId>::new());

    let built = SelectQuery::<MockDialect>::new()
        .from::<crate::schema::Aliased<Order, OrderAlias>>()
        .select((
            AliasedCol::<OrderAlias, OrderId>::new(),
            lag(AliasedCol::<OrderAlias, OrderTotal>::new())
                .offset(1)
                .over(window),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"o\".\"id\", LAG(\"o\".\"total\", 1) OVER (PARTITION BY \"o\".\"user_id\" \
         ORDER BY \"o\".\"id\" DESC) FROM \"orders\" AS \"o\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_window_ordering_can_fix_where_nulls_sort() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((
            OrderId,
            row_number()
                .over(Window::partition_by(OrderUserId).order_by_desc_nulls_last(OrderTotal)),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", ROW_NUMBER() OVER (PARTITION BY \"user_id\" \
         ORDER BY \"total\" DESC NULLS LAST) FROM \"orders\""
    );
    assert!(built.params.is_empty());
}
