//! Grouping, aggregate projections, and the grouped typestate.

use super::{
    MockBackend, MockDialect, Order, OrderId, OrderTotal, OrderUserId, User, UserId, UserName,
};
use crate::expr::{coalesce, Expr, SqlVal};
use crate::query::*;
use crate::schema::{At, Column, Next};

#[test]
fn group_by_renders_single_column_and_tuple() {
    let single = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .group_by(UserId)
        .select((UserId,))
        .build()
        .expect("valid query");
    assert_eq!(single.sql, "SELECT \"id\" FROM \"users\" GROUP BY \"id\"");

    let tuple = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .group_by((UserId, UserName))
        .select((UserId, UserName))
        .build()
        .expect("valid query");
    assert_eq!(
        tuple.sql,
        "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
    );
}

#[test]
fn multiple_group_by_calls_append_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .group_by(UserId)
        .group_by(UserName)
        .select((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
    );
}

#[test]
fn having_renders_aggregate_filters_and_binds_parameters() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .having(Expr::count_all().gt(5i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" HAVING (COUNT(*) > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(5)]);
}

#[test]
fn repeated_having_calls_are_combined_with_and() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .where_(Expr::column::<OrderId>().gt(0i64))
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .having(Expr::count_all().gt(2i64))
        .having(Expr::sum::<OrderTotal>().lt(100.0))
        .order_by_asc(OrderUserId)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" WHERE (\"id\" > $1) GROUP BY \"user_id\" \
         HAVING ((COUNT(*) > $2) AND (SUM(\"total\") < $3)) ORDER BY \"user_id\" ASC"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(0), SqlVal::I64(2), SqlVal::F64(100.0)]
    );
}

#[test]
fn group_by_transitions_to_the_grouped_typestate() {
    let query: GroupedQuery<MockDialect, Order, (i64,), GroupSet<EmptyGroupSet, OrderUserId>> =
        SelectQuery::<MockDialect>::new()
            .from::<Order>()
            .group_by(OrderUserId)
            .select((OrderUserId,));

    assert_eq!(
        query.build().expect("valid query").sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\""
    );
}

#[test]
fn grouped_selection_accepts_grouped_columns_and_aggregates() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((
            OrderUserId,
            Expr::count_all(),
            Expr::sum::<OrderTotal>(),
            Expr::avg::<OrderTotal>(),
            Expr::min::<OrderId>(),
            Expr::max::<OrderTotal>(),
        ))
        .having(Expr::count_all().gt(1i64))
        .order_by_asc(OrderUserId)
        .limit(5)
        .offset(2)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*), SUM(\"total\"), AVG(\"total\"), MIN(\"id\"), \
         MAX(\"total\") FROM \"orders\" GROUP BY \"user_id\" HAVING (COUNT(*) > $1) \
         ORDER BY \"user_id\" ASC LIMIT 5 OFFSET 2"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn chained_group_by_extends_the_grouped_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .group_by(UserId)
        .group_by(UserName)
        .select((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" GROUP BY \"id\", \"name\""
    );
}

#[test]
fn grouped_queries_order_and_filter_by_aggregates_and_grouped_columns() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()))
        .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
        .having(Expr::count_all().gt(1i64))
        .order_by_expr_desc(Expr::count_all())
        .order_by_expr_nulls(
            Expr::sum::<OrderTotal>(),
            OrderDirection::Asc,
            NullsOrder::Last,
        )
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING ((\"user_id\" > $1) AND (COUNT(*) > $2)) \
         ORDER BY COUNT(*) DESC, SUM(\"total\") ASC NULLS LAST"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10), SqlVal::I64(1)]);
}

#[test]
fn grouped_queries_are_executable() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()));
    let query_built = query.build().expect("valid query");

    let mut backend = MockBackend {
        rows: vec![vec![SqlVal::I64(1), SqlVal::I64(3)]],
        ..MockBackend::default()
    };

    let rows = backend.fetch_all(&query).expect("grouped rows decode");

    assert_eq!(rows, vec![(1i64, 3i64)]);
    assert_eq!(backend.executed, vec![query_built]);
}

#[test]
fn grouped_having_accepts_a_predicate_that_is_constant_within_a_group() {
    // A fragment comparing two aggregates reads no column of the query, so it is
    // constant within a group and filters the groups exactly like an aggregate
    // comparison does — without being an aggregate expression itself.
    let constant = Expr::<bool>::raw("SUM(total) > COUNT(*)", Vec::new())
        .expect("a template without placeholders");

    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .having(constant)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" HAVING SUM(total) > COUNT(*)"
    );
    assert!(built.params.is_empty());
}

#[test]
fn having_stays_recorded_in_the_grouped_statement() {
    // The `HAVING` clause is recorded on the grouped query itself, so the
    // rendered statement keeps the filter together with the grouping that decides
    // which values the predicate may read.
    let grouped = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .having(Expr::count_all().gt(1i64));

    let built = grouped.build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" HAVING (COUNT(*) > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn group_set_resolves_grouped_column_positions() {
    type Grouping = GroupSet<GroupSet<EmptyGroupSet, UserId>, UserName>;

    fn assert_grouped<G, C: Column, P>()
    where
        G: ColumnInGroupBy<C, P>,
    {
    }

    fn assert_grouped_selection<S, G, P>()
    where
        S: GroupedSelection<G, P>,
    {
    }

    assert_grouped::<Grouping, UserName, At<0>>();
    assert_grouped::<Grouping, UserId, Next<At<0>>>();
    assert_grouped_selection::<(UserId, UserName, Expr<i64>), Grouping, (Next<At<0>>, At<0>, At<0>)>(
    );
}

#[test]
fn order_by_expr_orders_the_groups_by_a_computed_aggregate_expression() {
    // `coalesce` keeps the aggregate marker of its first operand, so the whole
    // expression collapses the rows of a group and orders the grouped rows,
    // binding the parameters of its fallback along the way.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .order_by_expr_desc(coalesce(Expr::sum::<OrderTotal>(), 0.0))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" \
         ORDER BY COALESCE(SUM(\"total\"), $1) DESC"
    );
    assert_eq!(built.params, vec![SqlVal::F64(0.0)]);
}

#[test]
fn order_by_expr_renders_the_direction_and_nulls_flavours() {
    let ascending = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .order_by_expr_asc(Expr::count_all())
        .build()
        .expect("valid query");

    assert_eq!(
        ascending.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" ORDER BY COUNT(*) ASC"
    );

    let nulls_first = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .order_by_expr_desc_nulls_first(Expr::min::<OrderId>())
        .build()
        .expect("valid query");

    assert_eq!(
        nulls_first.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" \
         ORDER BY MIN(\"id\") DESC NULLS FIRST"
    );

    let nulls_last = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .order_by_expr_asc_nulls_last(Expr::max::<OrderTotal>())
        .build()
        .expect("valid query");

    assert_eq!(
        nulls_last.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" \
         ORDER BY MAX(\"total\") ASC NULLS LAST"
    );
}

/// `ABS("total")`, a per-row expression over one column.
fn abs_total() -> Expr<f64, Order> {
    Expr::<f64>::fragments("ABS(")
        .operand(OrderTotal)
        .text(")")
        .end()
}

#[test]
fn group_by_renders_a_computed_expression() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(abs_total())
        .select((abs_total(), Expr::count_all()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT ABS(\"total\"), COUNT(*) FROM \"orders\" GROUP BY ABS(\"total\")"
    );
    assert!(built.params.is_empty());
}

#[test]
fn having_reads_a_grouped_computed_expression() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(abs_total())
        .select((abs_total(), Expr::count_all()))
        .having(abs_total().gt(10.0))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT ABS(\"total\"), COUNT(*) FROM \"orders\" GROUP BY ABS(\"total\") \
         HAVING (ABS(\"total\") > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(10.0)]);
}

#[test]
fn group_by_chains_a_column_and_a_computed_expression() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .group_by(abs_total())
        .select((OrderUserId, abs_total(), Expr::count_all()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", ABS(\"total\"), COUNT(*) FROM \"orders\" \
         GROUP BY \"user_id\", ABS(\"total\")"
    );
}

#[test]
fn grouped_column_ordering_renders_every_direction_and_nulls_flavour() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .order_by_desc(OrderUserId)
        .order_by_asc_nulls_first(OrderUserId)
        .order_by_asc_nulls_last(OrderUserId)
        .order_by_desc_nulls_first(OrderUserId)
        .order_by_desc_nulls_last(OrderUserId)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" \
         ORDER BY \"user_id\" DESC, \"user_id\" ASC NULLS FIRST, \
         \"user_id\" ASC NULLS LAST, \"user_id\" DESC NULLS FIRST, \
         \"user_id\" DESC NULLS LAST"
    );
    assert!(built.params.is_empty());
}

#[test]
fn grouped_expression_ordering_renders_every_direction_and_nulls_flavour() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .order_by_expr_asc(Expr::count_all())
        .order_by_expr_desc(Expr::min::<OrderId>())
        .order_by_expr_asc_nulls_first(Expr::max::<OrderTotal>())
        .order_by_expr_asc_nulls_last(Expr::sum::<OrderTotal>())
        .order_by_expr_desc_nulls_first(Expr::avg::<OrderTotal>())
        .order_by_expr_desc_nulls_last(Expr::count_all())
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\" FROM \"orders\" GROUP BY \"user_id\" \
         ORDER BY COUNT(*) ASC, MIN(\"id\") DESC, MAX(\"total\") ASC NULLS FIRST, \
         SUM(\"total\") ASC NULLS LAST, AVG(\"total\") DESC NULLS FIRST, \
         COUNT(*) DESC NULLS LAST"
    );
    assert!(built.params.is_empty());
}

#[test]
fn grouped_ordering_takes_an_explicit_nulls_placement_and_paginates_the_groups() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by((OrderUserId, OrderId))
        .select((OrderUserId, OrderId, Expr::count_all()))
        .order_by_nulls(OrderId, OrderDirection::Desc, NullsOrder::First)
        .order_by_expr_nulls(
            Expr::sum::<OrderTotal>(),
            OrderDirection::Asc,
            NullsOrder::Last,
        )
        .limit(3)
        .offset(1)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", \"id\", COUNT(*) FROM \"orders\" \
         GROUP BY \"user_id\", \"id\" ORDER BY \"id\" DESC NULLS FIRST, \
         SUM(\"total\") ASC NULLS LAST LIMIT 3 OFFSET 1"
    );
    assert!(built.params.is_empty());
}
