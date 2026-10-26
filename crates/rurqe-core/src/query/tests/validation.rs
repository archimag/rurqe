//! Pre-flight validation of the assembled statement.

use super::{
    MockDialect, Order, OrderAlias, OrderId, OrderTotal, OrderUserId, RecentId, RecentOrder,
    RecentTotal, User, UserId, UserName,
};
use crate::expr::{
    BinaryOp, Expr, ExprKind, LockClause, LockStrength, OrderCriterion, OrderDirection, SelectAst,
    SqlVal, Subquery, SubqueryOp,
};
use crate::query::*;
use crate::schema::{Aliased, AliasedCol};

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

#[test]
fn an_aggregate_beside_an_ungrouped_column_is_rejected_before_rendering() {
    // An aggregate without a `GROUP BY` clause collapses the relation into one row,
    // so the bare column beside it names a value the statement never reports.
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderUserId, Expr::count_all()));

    assert_eq!(
        query.build(),
        Err(BuildError::UngroupedColumnWithAggregate {
            table: "orders",
            column: "user_id",
        })
    );
}

#[test]
fn a_grouped_aggregate_projection_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all(), Expr::sum::<OrderTotal>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*), SUM(\"total\") FROM \"orders\" GROUP BY \"user_id\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn an_aggregate_only_projection_without_group_by_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((Expr::count_all(),))
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "SELECT COUNT(*) FROM \"orders\"");
    assert!(built.params.is_empty());
}

#[test]
fn a_having_predicate_reading_a_grouped_column_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()))
        .having(Expr::count_all().gt(1i64))
        .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING ((COUNT(*) > $1) AND (\"user_id\" > $2))"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(10)]);
}

#[test]
fn a_having_predicate_reading_an_ungrouped_column_is_rejected_before_rendering() {
    // The grouping dropped the per-row value `total` would name, so the predicate
    // cannot read it: the columns of a group are the aggregated and the grouped ones.
    let query = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId,))
        .having_col(
            OrderUserId,
            Expr::column::<OrderUserId>()
                .gt(10i64)
                .and(Expr::column::<OrderTotal>().gt(100.0).without_scope()),
        );

    assert_eq!(
        query.build(),
        Err(BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_window_call_beside_a_bare_column_stays_valid_without_a_grouping() {
    // A window function is evaluated over the rows the statement produced, so it
    // neither collapses the relation nor requires a grouping.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((
            OrderUserId,
            Expr::sum::<OrderTotal>().over(Window::partition_by(OrderUserId)),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", SUM(\"total\") OVER (PARTITION BY \"user_id\") FROM \"orders\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_projection_qualified_by_the_declared_alias_is_accepted() {
    let built = SelectQuery::<MockDialect>::new()
        .from::<Aliased<Order, OrderAlias>>()
        .select((AliasedCol::<OrderAlias, OrderId>::new(),))
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "SELECT \"o\".\"id\" FROM \"orders\" AS \"o\"");
}

#[test]
fn a_projection_qualified_by_a_name_the_statement_does_not_declare_is_rejected() {
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    ast.projections = vec![ExprKind::QualifiedColumn {
        table: "o",
        column: "id",
    }];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "o" })
    );
}

#[test]
fn a_filter_qualified_by_a_name_the_statement_does_not_declare_is_rejected() {
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "o",
            column: "user_id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "id",
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "o" })
    );
}

#[test]
fn an_order_by_qualified_by_a_name_the_statement_does_not_declare_is_rejected() {
    let mut ast = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderId,))
        .to_ast();
    ast.order_by.push(OrderCriterion::new(
        ExprKind::QualifiedColumn {
            table: "o",
            column: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "o" })
    );
}

/// Builds a statement whose filter is a subquery over `recent` correlating a
/// reference qualified as `qualifier` to the enclosing `orders AS o` statement.
fn correlated_statement(qualifier: &'static str) -> SelectAst {
    let mut inner = SelectQuery::<MockDialect>::new()
        .from::<RecentOrder>()
        .select((RecentId,))
        .to_ast();
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "recent",
            name: "id",
        }),
        rhs: Box::new(ExprKind::QualifiedColumn {
            table: qualifier,
            column: "id",
        }),
    });

    let mut outer = SelectQuery::<MockDialect>::new()
        .from::<Aliased<Order, OrderAlias>>()
        .select((AliasedCol::<OrderAlias, OrderId>::new(),))
        .to_ast();
    outer.filter = Some(ExprKind::Subquery {
        op: SubqueryOp::Exists,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    });
    outer
}

#[test]
fn a_correlated_subquery_may_reference_the_alias_of_the_enclosing_statement() {
    // The nested statement introduces `recent` only, so `o.id` resolves through
    // the alias the statement enclosing it declares.
    assert_eq!(correlated_statement("o").validate(), Ok(()));
}

#[test]
fn a_correlated_subquery_qualified_by_a_name_nothing_declares_is_rejected() {
    assert_eq!(
        correlated_statement("ghosts").validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

/// A dialect exposing the upsert clause the DML validation tests exercise.
///
/// The `ON CONFLICT` builder is only reachable on a dialect implementing
/// [`SupportsOnConflict`](crate::dialect::SupportsOnConflict), so the DML
/// regressions are checked against this dialect instead of the mock one the
/// query tests share.
struct UpsertDialect;

impl crate::dialect::Dialect for UpsertDialect {
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

impl crate::dialect::SupportsOnConflict for UpsertDialect {}

#[test]
fn an_insert_value_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The assigned expression reads `recent.total` explicitly, which the `VALUES`
    // list of an `INSERT INTO "orders"` never declares.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderTotal::set_expr(
            Expr::column::<RecentTotal>().without_scope(),
        ))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn a_conflict_target_predicate_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .target_where(Expr::column::<RecentId>().without_scope().gt(0i64))
        .do_nothing()
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn a_conflict_update_predicate_qualified_by_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .update_where(Expr::column::<RecentTotal>().without_scope().gt(0.0f64))
        .do_update(OrderTotal::set(0.0f64))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn a_conflict_assignment_reading_a_foreign_relation_is_rejected() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .do_update(OrderTotal::set_expr(
            Expr::column::<RecentTotal>().without_scope(),
        ))
        .build();

    assert_eq!(
        built,
        Err(BuildError::UnresolvedQualifier { name: "recent" })
    );
}

#[test]
fn an_upsert_addressing_the_target_relation_and_its_proposed_row_is_accepted() {
    use crate::dml::{excluded, ColumnAssign, InsertQuery};

    // Both predicates qualify their references through the inserted table, and the
    // assignment copies the proposed row, which names no qualifier of its own.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values((OrderId::set(1i64), OrderTotal::set(2.5f64)))
        .on_conflict((OrderId,))
        .target_where(Expr::column::<OrderId>().without_scope().gt(0i64))
        .update_where(Expr::column::<OrderTotal>().without_scope().gt(0.0f64))
        .do_update(OrderTotal::set_expr(excluded(OrderTotal)))
        .build()
        .expect("the clause addresses the inserted table and its proposed row");

    assert_eq!(
        built.sql,
        "INSERT INTO \"orders\" (\"id\", \"total\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") WHERE (\"orders\".\"id\" > $3) \
         DO UPDATE SET \"total\" = EXCLUDED.\"total\" WHERE (\"orders\".\"total\" > $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::F64(2.5),
            SqlVal::I64(0),
            SqlVal::F64(0.0),
        ]
    );
}
