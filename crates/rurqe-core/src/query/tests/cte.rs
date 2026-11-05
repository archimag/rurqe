//! Common table expressions and the relations they introduce.

use super::{MockDialect, Order, OrderId, OrderTotal, OrderUserId, User, UserId, UserName};
use crate::expr::{Expr, SqlVal};
use crate::query::*;

#[test]
fn single_cte_renders_with_clause_and_binds_sequential_parameters() {
    let active = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64));
    let active_cte = cte::<User, _, _, _, _>(active);

    let built = SelectQuery::<MockDialect>::new()
        .with(active_cte)
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserName>().eq("root"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "WITH \"users\" AS (SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)) \
         SELECT \"id\", \"name\" FROM \"users\" WHERE (\"name\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(10), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn multiple_ctes_render_comma_separated_clauses() {
    let u_sub = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().gt(1i64));
    let o_sub = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .where_(Expr::column::<OrderTotal>().gt(100.0));

    let built = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", u_sub))
        .with(cte_named("large_orders", o_sub))
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().eq(7i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "WITH \"active_users\" AS (SELECT \"id\" FROM \"users\" WHERE (\"id\" > $1)), \
         \"large_orders\" AS (SELECT \"id\" FROM \"orders\" WHERE (\"total\" > $2)) \
         SELECT \"id\" FROM \"users\" WHERE (\"id\" = $3)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::F64(100.0), SqlVal::I64(7)]
    );
}

#[test]
fn cte_binds_to_table_marker_and_participates_in_joins() {
    crate::table!(
        ActiveSummary => "active_summary" {
            SummaryUserId => "user_id": i64,
            SummaryTotal => "total": f64,
        }
    );

    let summary_query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId, OrderTotal))
        .where_(Expr::column::<OrderTotal>().gt(50.0));

    let summary_cte = summary_query.as_cte::<ActiveSummary>();

    let built = SelectQuery::<MockDialect>::new()
        .with(summary_cte)
        .from::<User>()
        .inner_join(
            ActiveSummary,
            Expr::column::<UserId>().eq(Expr::column::<SummaryUserId>()),
        )
        .select((UserId, UserName, SummaryTotal))
        .where_(Expr::column::<SummaryTotal>().lt(500.0))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "WITH \"active_summary\" AS (SELECT \"user_id\", \"total\" FROM \"orders\" WHERE (\"total\" > $1)) \
         SELECT \"users\".\"id\", \"users\".\"name\", \"active_summary\".\"total\" FROM \"users\" \
         INNER JOIN \"active_summary\" ON (\"users\".\"id\" = \"active_summary\".\"user_id\") \
         WHERE (\"active_summary\".\"total\" < $2)"
    );
    assert_eq!(built.params, vec![SqlVal::F64(50.0), SqlVal::F64(500.0)]);
}

#[test]
fn a_cte_declared_twice_under_one_table_marker_is_rejected() {
    let first = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,));
    let second = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,));

    let query = SelectQuery::<MockDialect>::new()
        .with(first.as_cte::<User>())
        .with(second.as_cte::<User>())
        .from::<Order>()
        .select((OrderId,));

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateCte { name: "users" })
    );
}

#[test]
fn duplicate_cte_detection_reports_the_first_repetition_in_declaration_order() {
    let body = || {
        SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
    };

    // Two names repeat after several distinct ones; the reported name has to be
    // the first repetition the declaration order reaches, so the detection stays
    // a scan of the clause rather than a set of unordered hits.
    let query = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", body()))
        .with(cte_named("large_orders", body()))
        .with(cte_named("recent_orders", body()))
        .with(cte_named("archived_orders", body()))
        .with(cte_named("active_users", body()))
        .with(cte_named("recent_orders", body()))
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
fn a_cte_of_a_compound_statement_scopes_over_every_operand() {
    // The `WITH` clause belongs to the statement as a whole, so it renders ahead
    // of the combination and stays outside the parentheses of the left operand:
    // every operand of the union reads the declared relation, exactly as it does
    // without the set operation.
    let active = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().gt(10i64));
    let other = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserId>().lt(5i64));

    let built = SelectQuery::<MockDialect>::new()
        .with(cte_named("active_users", active))
        .from::<User>()
        .select((UserId,))
        .union(other)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "WITH \"active_users\" AS (SELECT \"id\" FROM \"users\" WHERE (\"id\" > $1)) \
         SELECT \"id\" FROM \"users\" UNION SELECT \"id\" FROM \"users\" WHERE (\"id\" < $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10), SqlVal::I64(5)]);
}
