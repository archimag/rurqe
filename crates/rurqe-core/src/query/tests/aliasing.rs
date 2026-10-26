//! Aliased relations, derived tables, and lateral subqueries.

use super::{
    latest_order_per_user_scope, ManagerAlias, MockDialect, Order, OrderAlias, OrderId, OrderTotal,
    OrderUserId, RecentId, RecentOrder, RecentTotal, User, UserAlias, UserId, UserName,
};
use crate::expr::{Expr, SqlVal};
use crate::query::*;
use crate::schema::{Aliased, AliasedCol};

#[test]
fn aliased_table_renders_as_clause_and_qualifies_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Aliased<User, UserAlias>>()
        .select((
            AliasedCol::<UserAlias, UserId>::new(),
            AliasedCol::<UserAlias, UserName>::new(),
        ))
        .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().gt(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"u\".\"id\", \"u\".\"name\" FROM \"users\" AS \"u\" WHERE (\"u\".\"id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn aliased_table_participates_in_joins() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join::<Aliased<Order, OrderAlias>, _, _>(
            Expr::column::<UserId>().eq(Expr::column::<AliasedCol<OrderAlias, OrderUserId>>()),
        )
        .select((UserId, AliasedCol::<OrderAlias, OrderTotal>::new()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"o\".\"total\" FROM \"users\" \
         INNER JOIN \"orders\" AS \"o\" ON (\"users\".\"id\" = \"o\".\"user_id\")"
    );
    assert!(built.params.is_empty());
}

#[test]
fn left_joined_aliased_table_lifts_columns_into_options() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join::<Aliased<Order, OrderAlias>, _, _>(
            Expr::column::<AliasedCol<OrderAlias, OrderUserId>>().eq(Expr::column::<UserId>()),
        );
    let total = query.nullable::<AliasedCol<OrderAlias, OrderTotal>>();

    let built = query.select((UserId, total)).build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"o\".\"total\" FROM \"users\" \
         LEFT JOIN \"orders\" AS \"o\" ON (\"o\".\"user_id\" = \"users\".\"id\")"
    );
}

#[test]
fn order_by_and_group_by_qualify_aliased_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Aliased<User, UserAlias>>()
        .inner_join::<Aliased<User, ManagerAlias>, _, _>(
            Expr::column::<AliasedCol<UserAlias, UserId>>()
                .eq(Expr::column::<AliasedCol<ManagerAlias, UserId>>()),
        )
        .group_by((
            AliasedCol::<UserAlias, UserId>::new(),
            AliasedCol::<ManagerAlias, UserName>::new(),
        ))
        .select((
            AliasedCol::<UserAlias, UserId>::new(),
            AliasedCol::<ManagerAlias, UserName>::new(),
        ))
        .order_by(AliasedCol::<UserAlias, UserId>::new(), OrderDirection::Asc)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"u\".\"id\", \"m\".\"name\" FROM \"users\" AS \"u\" \
         INNER JOIN \"users\" AS \"m\" ON (\"u\".\"id\" = \"m\".\"id\") \
         GROUP BY \"u\".\"id\", \"m\".\"name\" ORDER BY \"u\".\"id\" ASC"
    );
    assert!(built.params.is_empty());
}

#[test]
fn unaliased_table_reference_ignores_a_preceding_aliased_instance_of_the_same_table() {
    // The `FROM` relation is `"users" AS "m"`, and the joined relation is the
    // unaliased `"users"` table. A plain column reference to `User` must stay
    // qualified against `"users"`, not be rewritten into the `"m"` alias just
    // because the aliased instance is encountered first.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Aliased<User, ManagerAlias>>()
        .inner_join::<User, _, _>(
            Expr::column::<AliasedCol<ManagerAlias, UserId>>().eq(Expr::column::<UserId>()),
        )
        .select((UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\" FROM \"users\" AS \"m\" INNER JOIN \"users\" \
         ON (\"m\".\"id\" = \"users\".\"id\")"
    );
    assert!(built.params.is_empty());
}

#[test]
fn cross_join_lateral_renders_inline_correlated_subquery() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .cross_join_lateral(latest_order_per_user_scope().as_lateral::<RecentOrder>())
        .select((UserId, RecentTotal))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"recent\".\"total\" FROM \"users\" CROSS JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\") LIMIT 1) AS \"recent\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn left_join_lateral_renders_on_true_and_lifts_columns_into_options() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .left_join_lateral(latest_order_per_user_scope().as_lateral::<RecentOrder>());
    let total = query.nullable::<RecentTotal>();

    let built = query.select((UserId, total)).build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"recent\".\"total\" FROM \"users\" LEFT JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\") LIMIT 1) \
         AS \"recent\" ON true"
    );
}

#[test]
fn lateral_join_binds_subquery_parameters_before_the_outer_filter() {
    let sub = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,))
        .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>().without_scope()))
        .where_(Expr::column::<OrderTotal>().gt(10.0))
        .limit(1);

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .cross_join_lateral(sub.as_lateral::<RecentOrder>())
        .select((UserId, RecentTotal))
        .where_(Expr::column::<UserId>().gt(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"recent\".\"total\" FROM \"users\" CROSS JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" \
         WHERE ((\"user_id\" = \"users\".\"id\") AND (\"total\" > $1)) \
         LIMIT 1) AS \"recent\" WHERE (\"users\".\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(10.0), SqlVal::I64(1)]);
}

#[test]
fn an_aliased_marker_inlines_the_derived_subquery_it_registers() {
    // The derived relation is registered under the alias its marker exposes it
    // under, so the statement selects from the rows of the subquery instead of
    // silently reading the stored table of the same name.
    let derived = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,))
        .as_table::<Aliased<RecentOrder, OrderAlias>>();

    let built = SelectQuery::<MockDialect>::new()
        .from_subquery(derived)
        .select((Expr::column::<AliasedCol<OrderAlias, RecentTotal>>(),))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"o\".\"total\" FROM (SELECT \"total\" FROM \"orders\") AS \"o\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn an_aliased_derived_table_joins_under_its_alias() {
    // A derived table is materialised before the statement it is joined into, so
    // its body resolves against its own relations alone: the filter reads a column
    // of `orders` instead of reaching for the relation the join introduces.
    let derived = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, OrderTotal))
        .where_(Expr::column::<OrderTotal>().gt(10.0))
        .as_table::<Aliased<RecentOrder, OrderAlias>>();

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .inner_join_subquery(
            derived,
            Expr::column::<UserId>().eq(Expr::column::<AliasedCol<OrderAlias, RecentId>>()),
        )
        .select((UserId, Expr::column::<AliasedCol<OrderAlias, RecentTotal>>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"o\".\"total\" FROM \"users\" INNER JOIN \
         (SELECT \"id\", \"total\" FROM \"orders\" WHERE (\"total\" > $1)) AS \"o\" \
         ON (\"users\".\"id\" = \"o\".\"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::F64(10.0)]);
}

#[test]
fn an_aliased_marker_declares_the_lateral_relation_under_its_alias() {
    // The lateral relation is declared under the alias the marker addresses its
    // columns with, so the qualified references of its projection resolve.
    let latest = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,))
        .where_(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>().without_scope()))
        .limit(1);

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .cross_join_lateral(latest.as_lateral::<Aliased<RecentOrder, OrderAlias>>())
        .select((UserId, Expr::column::<AliasedCol<OrderAlias, RecentTotal>>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"o\".\"total\" FROM \"users\" CROSS JOIN LATERAL \
         (SELECT \"total\" FROM \"orders\" WHERE (\"user_id\" = \"users\".\"id\") LIMIT 1) AS \"o\""
    );
    assert!(built.params.is_empty());
}
