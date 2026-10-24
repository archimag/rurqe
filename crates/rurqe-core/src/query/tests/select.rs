//! Plain `SELECT` rendering, projections, and the value-based entrypoints.

use super::{
    compound_user_query, MockDialect, Order, OrderId, OrderTotal, OrderUserId, User, UserId,
    UserName,
};
use crate::expr::{greatest, is_distinct_from, is_not_distinct_from, least, Expr, SqlVal};
use crate::query::*;

#[test]
fn distinct_renders_distinct_keyword() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .distinct()
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "SELECT DISTINCT \"id\" FROM \"users\"");
}

#[test]
fn a_distinct_statement_orders_by_a_projected_expression() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserName,))
        .distinct()
        .order_by_asc(UserName)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT \"name\" FROM \"users\" ORDER BY \"name\" ASC"
    );
}

#[test]
fn a_distinct_statement_ordering_by_an_unprojected_expression_is_rejected() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .distinct()
        .order_by_desc(OrderTotal);

    assert_eq!(
        query.build(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_statement_reports_the_first_unprojected_ordering_criterion() {
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, OrderTotal))
        .distinct()
        .order_by_desc(OrderId)
        .order_by_asc(OrderUserId);

    assert_eq!(
        query.build(),
        Err(BuildError::DistinctOrderNotProjected { position: 1 })
    );
}

#[test]
fn a_distinct_compound_statement_keeps_its_compound_ordering_rule() {
    // The left operand renders with DISTINCT, but the ordering of the combined
    // rows names output columns, so it is the compound rule that rejects the
    // criterion — exactly as it does without the deduplication.
    assert_eq!(
        compound_user_query()
            .distinct()
            .union(compound_user_query())
            .order_by_output_asc(OrderTotal)
            .build(),
        Err(BuildError::CompoundOrderColumn { column: "total" })
    );
}

#[test]
fn distinct_on_renders_distinct_on_clause() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .distinct_on(UserId)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT DISTINCT ON (\"id\") \"id\", \"name\" FROM \"users\""
    );
}

#[test]
fn renders_selection_from_filter_and_pagination() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .limit(20)
        .offset(5)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1) LIMIT 20 OFFSET 5"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn selection_determines_row_type() {
    let query: SelectQuery<MockDialect, User, (i64,), HasProjection> =
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,));

    assert_eq!(
        query.build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn repeated_predicates_are_combined_with_and() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(1i64))
        .where_(Expr::column::<UserName>().eq("root"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE ((\"id\" > $1) AND (\"name\" = $2))"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn order_by_renders_asc_and_desc() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .order_by(UserId, OrderDirection::Desc)
        .order_by_asc(UserName)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" ORDER BY \"id\" DESC, \"name\" ASC"
    );
}

#[test]
fn the_projection_typestate_gates_the_statement_renderer() {
    fn assert_projected<P: ProjectionPresent>() {}

    // `HasProjection` is the only state whose queries reach the renderer, so a
    // query without a projection is rejected at compile time in every profile;
    // the `compile_fail` example on `SelectQuery::build` pins that contract.
    assert_projected::<HasProjection>();

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "SELECT \"id\" FROM \"users\"");
}

#[test]
fn value_table_argument_infers_the_from_table() {
    let built = SelectQuery::<MockDialect>::new()
        .from_table(User)
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn raw_sql_renders_sequential_placeholders_in_query_clauses() {
    let raw_pred: Expr<bool> = Expr::raw(
        "strftime('%Y-%m', date) = ?",
        vec![SqlVal::Text("2024-01".into())],
    )
    .expect("the template and its parameters match");

    let built = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((
            UserId,
            Expr::<i64>::raw("COUNT(*)", Vec::new()).expect("a template without placeholders"),
        ))
        .where_(Expr::column::<UserId>().gt(10i64))
        .where_(raw_pred)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", COUNT(*) FROM \"users\" WHERE ((\"id\" > $1) AND strftime('%Y-%m', date) = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(10), SqlVal::Text("2024-01".into())]
    );
}

#[test]
fn greatest_and_least_project_the_comparison_functions() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId, greatest(OrderTotal, [1.0, 2.0])))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\", GREATEST(\"total\", $1, $2) FROM \"orders\""
    );
    assert_eq!(built.params, vec![SqlVal::F64(1.0), SqlVal::F64(2.0)]);

    let lowest = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((least(OrderTotal, [1.0]),))
        .build()
        .expect("valid query");

    assert_eq!(lowest.sql, "SELECT LEAST(\"total\", $1) FROM \"orders\"");
    assert_eq!(lowest.params, vec![SqlVal::F64(1.0)]);
}

#[test]
fn the_null_safe_predicates_compare_absent_values() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(is_distinct_from(OrderTotal, 0.0))
        .where_(is_not_distinct_from(OrderUserId, 7i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"orders\" WHERE ((\"total\" IS DISTINCT FROM $1) AND \
         (\"user_id\" IS NOT DISTINCT FROM $2))"
    );
    assert_eq!(built.params, vec![SqlVal::F64(0.0), SqlVal::I64(7)]);
}
