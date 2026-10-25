//! Pre-flight validation of the assembled statement.

use super::{
    MockDialect, Order, OrderAlias, OrderId, OrderTotal, OrderUserId, User, UserId, UserName,
};
use crate::expr::{LockClause, LockStrength};
use crate::query::*;
use crate::schema::AliasedCol;

#[test]
fn distinct_on_qualifies_aliased_columns() {
    // PostgreSQL requires the leading ORDER BY criteria to repeat the
    // DISTINCT ON expressions, so the ordering begins with the user and only
    // then breaks ties by the total.
    let built = SelectQuery::<MockDialect>::new()
        .from::<crate::schema::Aliased<Order, OrderAlias>>()
        .select((
            AliasedCol::<OrderAlias, OrderId>::new(),
            AliasedCol::<OrderAlias, OrderTotal>::new(),
        ))
        .distinct_on(AliasedCol::<OrderAlias, OrderUserId>::new())
        .order_by_asc(AliasedCol::<OrderAlias, OrderUserId>::new())
        .order_by_asc(AliasedCol::<OrderAlias, OrderTotal>::new())
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"o\".\"user_id\") \"o\".\"id\", \"o\".\"total\" \
         FROM \"orders\" AS \"o\" ORDER BY \"o\".\"user_id\" ASC, \"o\".\"total\" ASC"
    );
}

#[test]
fn an_order_by_matching_the_distinct_on_prefix_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .distinct_on(OrderId)
        .order_by_desc(OrderUserId)
        .order_by_asc(OrderId)
        .order_by_desc(OrderTotal)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"user_id\", \"id\") \"id\" FROM \"orders\" \
         ORDER BY \"user_id\" DESC, \"id\" ASC, \"total\" DESC"
    );
}

#[test]
fn an_order_by_shorter_than_distinct_on_is_rejected_before_rendering() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .distinct_on(OrderId)
        .order_by_asc(OrderUserId);

    assert_eq!(
        query.build(),
        Err(BuildError::DistinctOnOrderMismatch { position: 1 })
    );
}

#[test]
fn a_mismatched_leading_order_by_criterion_is_rejected_before_rendering() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .order_by_asc(OrderTotal)
        .order_by_asc(OrderUserId);

    assert_eq!(
        query.build(),
        Err(BuildError::DistinctOnOrderMismatch { position: 0 })
    );
}

#[test]
fn distinct_on_without_order_by_stays_valid() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct_on(OrderUserId)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"user_id\") \"id\" FROM \"orders\""
    );
}

#[test]
fn a_distinct_on_statement_requesting_a_row_lock_is_rejected_before_rendering() {
    // The builder cannot reach this statement — `distinct_on` moves the query out
    // of `NoLock` — so the clause pair is assembled on the statement data, which
    // is what a statement embedded as plain AST still describes.
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    let projections = ast.projections.clone();
    ast.distinct_on = projections;
    ast.lock = Some(LockClause::new(LockStrength::Update));

    assert_eq!(ast.validate(), Err(BuildError::DistinctOnWithLock));
}

#[test]
fn a_locked_statement_without_distinct_on_stays_valid() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .for_update()
        .skip_locked()
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" FOR UPDATE SKIP LOCKED"
    );
}

#[test]
fn a_duplicate_named_cte_is_rejected_before_rendering() {
    let first = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName));
    let second = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let query = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", first))
        .with(cte_named("active_users", second))
        .from::<User>()
        .select((UserId,));

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateCte {
            name: "active_users"
        })
    );
}

#[test]
fn two_differently_named_ctes_are_accepted() {
    let active = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName));
    let large = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,));

    let built = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", active))
        .with(cte_named("large_orders", large))
        .from::<User>()
        .select((UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "WITH \"active_users\" AS (SELECT \"id\", \"name\" FROM \"users\"), \
         \"large_orders\" AS (SELECT \"id\" FROM \"orders\") SELECT \"id\" FROM \"users\""
    );
}
