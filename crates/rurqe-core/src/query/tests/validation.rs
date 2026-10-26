//! Pre-flight validation of the assembled statement.

use super::{
    MockDialect, Order, OrderAlias, OrderId, OrderTotal, OrderUserId, RecentId, RecentOrder,
    RecentTotal, User, UserId, UserName,
};
use crate::expr::{
    AggregateFunc, BinaryOp, CaseBranch, CteAst, DerivedAst, Expr, ExprKind, JoinAst, JoinKind,
    LockClause, LockStrength, OrderCriterion, OrderDirection, RawSqlError, SelectAst, SetOpAst,
    SetOpKind, SqlVal, Subquery, SubqueryOp,
};
use crate::query::validate::{
    collect_bare_columns, contains_bare_column, has_aggregate, register_relation,
    reject_shadowed_derived, reject_unresolved_qualifiers,
};
use crate::query::*;
use crate::schema::{Aliased, AliasedCol, TableRef};

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

// -----------------------------------------------------------------------------
// Section coverage of the consolidated walk.
//
// One enumeration describes the sections of a statement, so every rule of the
// pass reaches every section by construction. The tests below pin that down
// section by section: each one is reached by the structural rules and by the
// qualifier resolution alike, and each embedded statement is handed the relation
// namespace its own SQL clause grants it — the closed one for a `WITH` body, a
// derived table, and a set-operation operand, and the enclosing one for a
// correlated subquery and a lateral join.
// -----------------------------------------------------------------------------

/// Builds a statement over `table` projecting its `column`.
fn relation_statement(table: &'static str, column: &'static str) -> SelectAst {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain(table));
    ast.projections.push(ExprKind::Column {
        table,
        name: column,
    });
    ast
}

/// Builds a statement projecting a reference qualified by `qualifier` alone.
fn qualified_body(qualifier: &'static str) -> SelectAst {
    let mut ast = empty_ast();
    ast.projections.push(ExprKind::QualifiedColumn {
        table: qualifier,
        column: "id",
    });
    ast
}

/// Builds a lateral subquery over `orders` correlating a reference qualified by
/// `qualifier` to the statement joining it.
fn lateral_body(qualifier: &'static str) -> SelectAst {
    let mut ast = relation_statement("orders", "id");
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
        rhs: Box::new(ExprKind::QualifiedColumn {
            table: qualifier,
            column: "id",
        }),
    });
    ast
}

/// Builds an `EXISTS (...)` predicate around `inner`.
fn exists_subquery(inner: SelectAst) -> ExprKind {
    ExprKind::Subquery {
        op: SubqueryOp::Exists,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    }
}

/// A raw expression whose fragment/parameter census does not balance.
fn unbalanced_raw() -> ExprKind {
    ExprKind::Raw {
        fragments: vec!["1".to_owned(), " + 1".to_owned()],
        params: Vec::new(),
    }
}

/// The error an unbalanced raw expression is reported through.
fn unbalanced_raw_error() -> BuildError {
    BuildError::RawSql(RawSqlError::PlaceholderCount {
        expected: 1,
        provided: 0,
    })
}

#[test]
fn a_join_on_predicate_qualified_by_a_name_nothing_declares_is_rejected() {
    // The `ON` condition of a join belongs to the statement scope, so it resolves
    // its references against the relations the statement introduces.
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::Inner,
        table: TableRef::plain("orders"),
        lateral: None,
        on: Some(ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::QualifiedColumn {
                table: "ghosts",
                column: "id",
            }),
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_join_on_predicate_carrying_an_unbalanced_raw_expression_is_rejected() {
    // The structural rules reach the same section the qualifier pass reaches, so
    // an unbalanced raw expression is caught inside an `ON` condition as well.
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::Inner,
        table: TableRef::plain("orders"),
        lateral: None,
        on: Some(unbalanced_raw()),
    });

    assert_eq!(ast.validate(), Err(unbalanced_raw_error()));
}

#[test]
fn a_lateral_subquery_reads_the_relations_of_the_statement_joining_it() {
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::LeftLateral,
        table: TableRef::plain("recent"),
        lateral: Some(lateral_body("users")),
        on: None,
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_lateral_subquery_qualified_by_a_name_nothing_declares_is_rejected() {
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::LeftLateral,
        table: TableRef::plain("recent"),
        lateral: Some(lateral_body("ghosts")),
        on: None,
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_common_table_expression_resolves_its_own_relations() {
    let mut ast = relation_statement("orders", "id");
    ast.ctes.push(CteAst {
        name: "active_users",
        query: relation_statement("users", "id"),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_common_table_expression_correlates_with_no_enclosing_statement() {
    // The `WITH` body is validated against its own relations alone: the engine
    // correlates none of it to the statement declaring it.
    let mut ast = relation_statement("orders", "id");
    ast.ctes.push(CteAst {
        name: "active_users",
        query: qualified_body("users"),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
}

#[test]
fn a_derived_table_resolves_its_own_relations() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("totals"));
    ast.projections.push(ExprKind::Column {
        table: "totals",
        name: "id",
    });
    ast.derived.push(DerivedAst {
        name: "totals",
        query: relation_statement("orders", "total"),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_derived_table_correlates_with_no_enclosing_statement() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("totals"));
    ast.projections.push(ExprKind::Column {
        table: "totals",
        name: "id",
    });
    ast.derived.push(DerivedAst {
        name: "totals",
        query: qualified_body("users"),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
}

#[test]
fn a_set_operation_operand_resolves_its_own_relations() {
    let mut ast = relation_statement("orders", "id");
    ast.set_operations.push(SetOpAst {
        kind: SetOpKind::UnionAll,
        query: relation_statement("archived", "id"),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_set_operation_operand_correlates_with_no_enclosing_statement() {
    // An operand of `UNION ALL` is validated with the closed scope as well, so it
    // cannot reach a relation of the statement combining it.
    let mut ast = relation_statement("orders", "id");
    ast.set_operations.push(SetOpAst {
        kind: SetOpKind::UnionAll,
        query: qualified_body("users"),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
}

#[test]
fn a_group_by_expression_qualified_by_a_name_nothing_declares_is_rejected() {
    let mut ast = relation_statement("orders", "user_id");
    ast.group_by.push(ExprKind::QualifiedColumn {
        table: "ghosts",
        column: "id",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_distinct_on_expression_qualified_by_a_name_nothing_declares_is_rejected() {
    let mut ast = relation_statement("orders", "id");
    ast.distinct_on.push(ExprKind::QualifiedColumn {
        table: "ghosts",
        column: "id",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_having_predicate_qualified_by_a_name_nothing_declares_is_rejected() {
    // The aggregate reads the foreign reference, so the aggregation consistency
    // leaves the clause alone and the qualifier rule is the one that reports it.
    let mut ast = relation_statement("orders", "user_id");
    ast.group_by.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: Some(Box::new(ExprKind::QualifiedColumn {
                table: "ghosts",
                column: "id",
            })),
            filter: None,
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_having_predicate_carrying_an_unbalanced_raw_expression_is_rejected() {
    let mut ast = relation_statement("orders", "user_id");
    ast.group_by.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.having = Some(unbalanced_raw());

    assert_eq!(ast.validate(), Err(unbalanced_raw_error()));
}

#[test]
fn an_order_by_criterion_carrying_an_unbalanced_raw_expression_is_rejected() {
    let mut ast = relation_statement("orders", "id");
    ast.order_by.push(OrderCriterion::new(
        unbalanced_raw(),
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Err(unbalanced_raw_error()));
}

#[test]
fn a_structural_violation_inside_a_nested_statement_of_a_projection_is_reported() {
    // The structural rules reach the statements a clause embeds, so a violation
    // nested one level deeper surfaces through the same typed error.
    let mut inner = relation_statement("orders", "id");
    inner.projections.push(unbalanced_raw());

    let mut ast = relation_statement("users", "id");
    ast.projections.push(scalar_subquery(inner));

    assert_eq!(ast.validate(), Err(unbalanced_raw_error()));
}

#[test]
fn a_doubly_nested_subquery_reaches_the_outermost_relation_by_correlation() {
    // The enclosing namespace accumulates statement by statement, so a reference
    // nested two levels deep still resolves against the outermost relation.
    let innermost = qualified_body("users");

    let mut middle = relation_statement("orders", "id");
    middle.projections.push(scalar_subquery(innermost));

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(middle));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_doubly_nested_subquery_qualified_by_a_name_nothing_declares_is_rejected() {
    let innermost = qualified_body("ghosts");

    let mut middle = relation_statement("orders", "id");
    middle.projections.push(scalar_subquery(innermost));

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(middle));

    assert_eq!(
        ast.validate(),
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
fn an_insert_value_addresses_no_relation_at_all() {
    use crate::dml::{ColumnAssign, InsertQuery};

    // The `VALUES` list of an `INSERT INTO "orders"` renders the values of one row
    // and declares no relation, so even an expression reading `recent.total`
    // explicitly names a table the statement cannot resolve. The builder keeps a
    // column-bearing expression out of the clause at compile time, so the check is
    // exercised on a tree assembled past the type system.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderTotal::set_expr(Expr::<f64>::from_kind(
            ExprKind::QualifiedColumn {
                table: "recent",
                column: "total",
            },
        )))
        .build();

    assert_eq!(
        built,
        Err(BuildError::ColumnWithoutRelation {
            table: "recent",
            column: "total",
        })
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

// -----------------------------------------------------------------------------
// Rule coverage assembled on raw statement data.
//
// The tests below build a `SelectAst` directly, so they exercise the rules the
// fluent builders keep unreachable and the exact diagnostics those rules report.
// -----------------------------------------------------------------------------

#[test]
fn build_error_display_duplicate_relation() {
    let err = BuildError::DuplicateRelation { name: "users" };
    let msg = err.to_string();
    assert!(msg.contains("duplicate unaliased relation `users`"));
    assert!(msg.contains("table_alias!"));
}

#[test]
fn build_error_display_duplicate_cte() {
    let err = BuildError::DuplicateCte {
        name: "active_users",
    };
    let msg = err.to_string();
    assert!(msg.contains("duplicate CTE name `active_users`"));
    assert!(msg.contains("cte_named"));
}

#[test]
fn a_statement_declaring_one_cte_name_twice_is_rejected() {
    let mut ast = empty_ast();
    ast.ctes.push(cte("active_users"));
    ast.ctes.push(cte("active_users"));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DuplicateCte {
            name: "active_users"
        })
    );
}

#[test]
fn a_statement_declaring_differently_named_ctes_is_accepted() {
    let mut ast = empty_ast();
    ast.ctes.push(cte("active_users"));
    ast.ctes.push(cte("large_orders"));

    assert_eq!(ast.validate(), Ok(()));
}

/// Declares an empty statement as a Common Table Expression named `name`.
fn cte(name: &'static str) -> CteAst {
    CteAst {
        name,
        query: empty_ast(),
    }
}

#[test]
fn build_error_display_distinct_on_mismatch() {
    let err = BuildError::DistinctOnOrderMismatch { position: 1 };
    let msg = err.to_string();
    assert!(msg.contains("ORDER BY criterion at position 1"));
    assert!(msg.contains("DISTINCT ON"));
}

#[test]
fn build_error_display_distinct_order_not_projected() {
    let err = BuildError::DistinctOrderNotProjected { position: 1 };
    let msg = err.to_string();
    assert!(msg.contains("ORDER BY criterion at position 1"));
    assert!(msg.contains("must appear in the select list"));
}

#[test]
fn a_distinct_statement_ordering_a_projected_expression_is_accepted() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_statement_ordering_an_unprojected_expression_is_rejected() {
    let mut ast = empty_ast();
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_ordering_matches_an_aliased_projection_and_a_bare_position() {
    // The ordering criterion matches the expression the aliased projection
    // wraps, and a bare 1-based position addresses the first projection.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Aliased {
        expr: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        alias: "net_total",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Desc,
    ));
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["1".to_owned()],
            params: Vec::new(),
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_matches_a_column_addressed_through_the_other_node_shape() {
    // A projection built from a schema marker and an ordering built through a
    // table alias name the same column of the same relation, so both shapes
    // resolve to one canonical reference. The statement declares the relation
    // both shapes name, so the qualifier of the ordering resolves against it.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::QualifiedColumn {
            table: "orders",
            column: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_matches_an_aliased_computed_projection() {
    // The projection is exposed under an alias and computes a `CAST` over a
    // column marker, while the ordering names the same computation over the
    // alias-qualified shape of that column. The statement declares the relation
    // both shapes name, so the qualifier of the ordering resolves against it.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(ExprKind::Aliased {
        expr: Box::new(ExprKind::Cast {
            operand: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            target: "numeric",
        }),
        alias: "net_total",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Cast {
            operand: Box::new(ExprKind::QualifiedColumn {
                table: "orders",
                column: "total",
            }),
            target: "numeric",
        },
        OrderDirection::Desc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_ordering_of_another_computation_is_rejected() {
    // The projection is computed as numeric while the ordering computes text,
    // so the two expressions differ even though both read the same column.
    let mut ast = empty_ast();
    ast.distinct = true;
    ast.projections.push(ExprKind::Cast {
        operand: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        target: "numeric",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Cast {
            operand: Box::new(ExprKind::QualifiedColumn {
                table: "orders",
                column: "total",
            }),
            target: "text",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOrderNotProjected { position: 0 })
    );
}

#[test]
fn a_distinct_on_prefix_matches_an_order_by_written_through_the_other_node_shape() {
    // `DISTINCT ON` spells the column as an alias-qualified reference while the
    // leading ordering criterion reads the schema-marker shape of the same column
    // of the same relation. Both name one value, so the prefix alignment holds and
    // the statement built past the fluent builders stays valid.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.distinct_on.push(ExprKind::QualifiedColumn {
        table: "orders",
        column: "user_id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_distinct_on_prefix_naming_another_column_is_still_rejected() {
    // The canonical comparison normalises the two spellings of one column; it does
    // not make distinct columns match, so the alignment rule still rejects a leading
    // ordering criterion naming a column the clause does not deduplicate by.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    ast.distinct_on.push(ExprKind::QualifiedColumn {
        table: "orders",
        column: "user_id",
    });
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::DistinctOnOrderMismatch { position: 0 })
    );
}

#[test]
fn build_error_display_compound_order_index() {
    let err = BuildError::CompoundOrderIndex {
        index: 3,
        columns: 2,
    };
    let msg = err.to_string();
    assert!(msg.contains("ORDER BY position 3"));
    assert!(msg.contains("2 projected column(s)"));
    assert!(msg.contains("between 1 and 2"));
}

#[test]
fn build_error_display_compound_order_column() {
    let err = BuildError::CompoundOrderColumn { column: "total" };
    let msg = err.to_string();
    assert!(msg.contains("`total`"));
    assert!(msg.contains("output column name or a 1-based position"));
}

#[test]
fn build_error_display_compound_order_expression() {
    let msg = BuildError::CompoundOrderExpression.to_string();
    assert!(msg.contains("names an expression"));
    assert!(msg.contains("output column name or a 1-based position"));
}

#[test]
fn build_error_display_column_without_relation() {
    let err = BuildError::ColumnWithoutRelation {
        table: "orders",
        column: "total",
    };
    let msg = err.to_string();
    assert!(msg.contains("`orders.total`"));
    assert!(msg.contains("addresses no relation"));
}

#[test]
fn a_compound_ordering_by_an_aggregate_is_rejected() {
    // An aggregate collapses the rows of a group, so it names no output column
    // the combined statement produced.
    let mut ast = empty_ast();
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.compound_order_by.push(OrderCriterion::new(
        ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: None,
            filter: None,
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Err(BuildError::CompoundOrderExpression));
}

#[test]
fn a_compound_ordering_by_a_multi_fragment_raw_expression_is_rejected() {
    // The leading fragment parses as a number, but the expression continues, so
    // it is a computed value rather than a 1-based position.
    let mut ast = empty_ast();
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.compound_order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["1".to_owned(), " + 1".to_owned()],
            params: Vec::new(),
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Err(BuildError::CompoundOrderExpression));
}

#[test]
fn a_compound_ordering_by_a_single_fragment_position_is_accepted() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    ast.compound_order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["1".to_owned()],
            params: Vec::new(),
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_compound_ordering_by_a_qualified_column_is_rejected() {
    // The ordering of a compound statement sorts the rows the combination
    // produced, so it addresses no relation: a reference qualified through a table
    // names a relation the combined statement no longer declares, even when the
    // projection list exposes a column of that name.
    let mut ast = empty_ast();
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });
    ast.compound_order_by.push(OrderCriterion::new(
        ExprKind::QualifiedColumn {
            table: "orders",
            column: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::ColumnWithoutRelation {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn build_error_display_raw_sql() {
    let err = BuildError::RawSql(RawSqlError::PlaceholderCount {
        expected: 2,
        provided: 1,
    });
    let msg = err.to_string();
    assert!(msg.contains("invalid raw SQL expression"));
    assert!(msg.contains("expects 2 parameter(s), but 1 were provided"));
}

#[test]
fn an_unbalanced_raw_expression_is_rejected_before_rendering() {
    // Two fragments with no parameter would place a bound placeholder after
    // the first fragment and then continue with literal text, so the
    // statement is rejected before a single character of it is rendered.
    let mut ast = empty_ast();
    ast.projections.push(ExprKind::Raw {
        fragments: vec!["1".to_owned(), " + 1".to_owned()],
        params: Vec::new(),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::RawSql(RawSqlError::PlaceholderCount {
            expected: 1,
            provided: 0,
        }))
    );
}

#[test]
fn build_error_display_aliased_derived_relation() {
    let err = BuildError::AliasedDerivedRelation { name: "o" };
    let msg = err.to_string();
    assert!(msg.contains("addressed as `o`"));
    assert!(msg.contains("registered under another name"));
}

/// Builds a statement AST without any clause, used as the body of a derived
/// subquery in the registration checks below.
fn empty_ast() -> SelectAst {
    SelectAst {
        ctes: Vec::new(),
        distinct: false,
        distinct_on: Vec::new(),
        projections: Vec::new(),
        from: None,
        joins: Vec::new(),
        derived: Vec::new(),
        filter: None,
        group_by: Vec::new(),
        having: None,
        order_by: Vec::new(),
        limit: None,
        offset: None,
        set_operations: Vec::new(),
        compound_order_by: Vec::new(),
        compound_limit: None,
        compound_offset: None,
        lock: None,
    }
}

/// Registers a derived subquery under `name`.
fn derived(name: &'static str) -> DerivedAst {
    DerivedAst {
        name,
        query: empty_ast(),
    }
}

#[test]
fn a_derived_relation_registered_under_the_base_name_of_its_alias_is_rejected() {
    assert_eq!(
        reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("recent")]),
        Err(BuildError::AliasedDerivedRelation { name: "o" })
    );
}

#[test]
fn a_derived_relation_registered_under_the_name_it_is_addressed_by_is_accepted() {
    assert_eq!(
        reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("o")]),
        Ok(())
    );
    assert_eq!(
        reject_shadowed_derived(&TableRef::plain("recent"), &[derived("recent")]),
        Ok(())
    );
    assert_eq!(
        reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[]),
        Ok(())
    );
}

#[test]
fn registering_one_relation_twice_reports_the_effective_name() {
    let mut relations = Vec::new();
    assert_eq!(
        register_relation(&mut relations, TableRef::plain("users")),
        Ok(())
    );
    assert_eq!(
        register_relation(&mut relations, TableRef::plain("users")),
        Err(BuildError::DuplicateRelation { name: "users" })
    );
}

#[test]
fn alias_distinguished_occurrences_of_one_table_are_accepted() {
    let mut relations = Vec::new();
    assert_eq!(
        register_relation(&mut relations, TableRef::plain("accounts")),
        Ok(())
    );
    assert_eq!(
        register_relation(&mut relations, TableRef::aliased("accounts", "a")),
        Ok(())
    );
    // A reference resolves through the alias, so reusing it collides even
    // though the underlying tables differ.
    assert_eq!(
        register_relation(&mut relations, TableRef::aliased("customers", "a")),
        Err(BuildError::DuplicateRelation { name: "a" })
    );
}

#[test]
fn build_error_display_unresolved_qualifier() {
    let err = BuildError::UnresolvedQualifier { name: "u" };
    let msg = err.to_string();
    assert!(msg.contains("qualified as `u`"));
    assert!(msg.contains("`Table::table_ref`"));
}

#[test]
fn a_reference_qualified_by_a_declared_relation_name_is_accepted() {
    let relations = [TableRef::aliased("users", "u"), TableRef::plain("accounts")];

    // The alias of the target and the plain name of a `FROM` source are
    // both declared by the rendered statement.
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::QualifiedColumn {
                table: "u",
                column: "id",
            },
            &relations,
            &[],
        ),
        Ok(())
    );
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::QualifiedColumn {
                table: "accounts",
                column: "id",
            },
            &relations,
            &[],
        ),
        Ok(())
    );

    // A plain column reference states no qualifier of its own: the
    // renderer resolves the qualifier it needs against the declared
    // relations.
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::Column {
                table: "users",
                name: "id",
            },
            &relations,
            &[],
        ),
        Ok(())
    );

    // The walk reaches a qualifier nested inside a composite expression.
    let filter = ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "ghosts",
            column: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "users",
            name: "id",
        }),
    };
    assert_eq!(
        reject_unresolved_qualifiers(&filter, &relations, &[]),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_reference_qualified_by_the_base_name_of_an_aliased_relation_is_rejected() {
    let relations = [TableRef::aliased("users", "u")];

    // The relation is only reachable as `u`, so a reference spelled
    // through its base table name resolves to nothing the statement
    // renders.
    assert_eq!(
        reject_unresolved_qualifiers(
            &ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            },
            &relations,
            &[],
        ),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
}

/// Builds a scalar subquery around `inner`.
fn scalar_subquery(inner: SelectAst) -> ExprKind {
    ExprKind::Subquery {
        op: SubqueryOp::Scalar,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    }
}

#[test]
fn a_correlated_subquery_may_reference_a_relation_of_the_enclosing_statement() {
    // The nested statement reads `u.id` without introducing the relation: a
    // correlated reference reaches the enclosing statement, whose relations
    // are in scope where the subquery is written.
    let mut inner = empty_ast();
    inner.from = Some(TableRef::plain("orders"));
    inner.projections.push(ExprKind::QualifiedColumn {
        table: "u",
        column: "id",
    });
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[TableRef::aliased("users", "u")],
            &[],
        ),
        Ok(())
    );
}

#[test]
fn an_outer_relation_stays_reachable_from_a_nested_statement() {
    // The relation belongs to a statement enclosing the validated one, so it
    // is only reachable through the outer scope the walk carries along.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::QualifiedColumn {
        table: "u",
        column: "id",
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[],
            &[TableRef::aliased("users", "u")],
        ),
        Ok(())
    );
}

#[test]
fn a_nested_statement_qualified_by_a_name_nothing_declares_is_rejected() {
    let mut inner = empty_ast();
    inner.from = Some(TableRef::plain("orders"));
    inner.projections.push(ExprKind::QualifiedColumn {
        table: "ghosts",
        column: "id",
    });

    assert_eq!(
        reject_unresolved_qualifiers(
            &scalar_subquery(inner),
            &[TableRef::aliased("users", "u")],
            &[],
        ),
        Err(BuildError::UnresolvedQualifier { name: "ghosts" })
    );
}

#[test]
fn a_nested_collection_resolves_a_correlated_reference() {
    let mut inner = empty_ast();
    inner.from = Some(TableRef::plain("orders"));
    inner.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
    });

    let collection = ExprKind::Multiset {
        query: Subquery::new(inner),
        fields: vec!["id"],
        single: false,
    };

    assert_eq!(
        reject_unresolved_qualifiers(&collection, &[TableRef::aliased("users", "u")], &[]),
        Ok(())
    );
}

#[test]
fn build_error_display_ungrouped_column_with_aggregate() {
    let err = BuildError::UngroupedColumnWithAggregate {
        table: "orders",
        column: "status",
    };
    let msg = err.to_string();
    assert!(msg.contains("`orders.status`"));
    assert!(msg.contains("beside an aggregate"));
    assert!(msg.contains("GROUP BY"));
}

#[test]
fn build_error_display_ungrouped_having_column() {
    let err = BuildError::UngroupedHavingColumn {
        table: "orders",
        column: "status",
    };
    let msg = err.to_string();
    assert!(msg.contains("`orders.status`"));
    assert!(msg.contains("HAVING predicate reads column"));
    assert!(msg.contains("`.group_by(...)`"));
}

/// Builds a `SUM(table.name)` aggregate over one column.
fn sum_of(table: &'static str, name: &'static str) -> ExprKind {
    ExprKind::Aggregate {
        func: AggregateFunc::Sum,
        distinct: false,
        arg: Some(Box::new(ExprKind::Column { table, name })),
        filter: None,
    }
}

#[test]
fn an_aggregate_nested_in_a_function_is_detected() {
    let expr = ExprKind::Function {
        name: "COALESCE",
        args: vec![sum_of("orders", "total"), ExprKind::Value(SqlVal::I64(0))],
    };

    assert!(has_aggregate(&expr));
    assert!(!contains_bare_column(&expr));
}

#[test]
fn a_column_read_beside_an_aggregate_is_collected() {
    let expr = ExprKind::Binary {
        op: BinaryOp::Add,
        lhs: Box::new(sum_of("orders", "total")),
        rhs: Box::new(ExprKind::QualifiedColumn {
            table: "o",
            column: "tax",
        }),
    };
    let mut columns = Vec::new();
    collect_bare_columns(&expr, &mut columns);

    // The column the aggregate reads stays inside the aggregate, so only the
    // reference the expression computes beside it is collected — and it is
    // collected in the canonical `(table, column)` form both node shapes
    // resolve to.
    assert_eq!(columns, vec![("o", "tax")]);
    assert!(contains_bare_column(&expr));
}

#[test]
fn an_expression_over_aggregates_collects_no_bare_column() {
    let expr = ExprKind::Case {
        branches: vec![CaseBranch {
            condition: ExprKind::Binary {
                op: BinaryOp::Gt,
                lhs: Box::new(sum_of("orders", "total")),
                rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
            },
            result: sum_of("orders", "tax"),
        }],
        else_branch: Some(Box::new(ExprKind::Value(SqlVal::I64(0)))),
    };
    let mut columns = Vec::new();
    collect_bare_columns(&expr, &mut columns);

    assert!(columns.is_empty());
    assert!(!contains_bare_column(&expr));
    assert!(has_aggregate(&expr));
}

#[test]
fn a_nested_statement_hides_its_aggregates_from_the_enclosing_expression() {
    // The aggregate belongs to the nested statement and collapses the rows of
    // its own `FROM` scope, so it does not make the projection it is embedded
    // into an aggregate.
    let mut inner = empty_ast();
    inner.projections.push(sum_of("orders", "total"));
    let expr = ExprKind::Subquery {
        op: SubqueryOp::Scalar,
        operand: None,
        query: Subquery::new(inner),
        negated: false,
    };

    assert!(!has_aggregate(&expr));
    assert!(!contains_bare_column(&expr));
}

/// Builds a statement projecting `projections` from the `orders` table.
fn orders_statement(projections: Vec<ExprKind>) -> SelectAst {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.projections = projections;
    ast
}

/// A `COUNT(*)` aggregate expression.
fn count_star() -> ExprKind {
    ExprKind::Aggregate {
        func: AggregateFunc::Count,
        distinct: false,
        arg: None,
        filter: None,
    }
}

#[test]
fn an_aggregate_beside_a_bare_column_is_rejected() {
    let ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        sum_of("orders", "total"),
    ]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumnWithAggregate {
            table: "orders",
            column: "user_id",
        })
    );
}

#[test]
fn an_aggregate_beside_an_alias_qualified_column_is_rejected() {
    // A reference built through a table alias names a column of a relation like
    // the schema marker does, so it is reported through the canonical pair.
    let ast = orders_statement(vec![
        sum_of("orders", "total"),
        ExprKind::QualifiedColumn {
            table: "o",
            column: "user_id",
        },
    ]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumnWithAggregate {
            table: "o",
            column: "user_id",
        })
    );
}

#[test]
fn a_statement_projecting_aggregates_and_literals_is_accepted() {
    let ast = orders_statement(vec![
        sum_of("orders", "total"),
        count_star(),
        ExprKind::Value(SqlVal::I64(1)),
    ]);

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn an_expression_over_an_aggregate_is_accepted_beside_it() {
    let ast = orders_statement(vec![
        sum_of("orders", "total"),
        ExprKind::Function {
            name: "COALESCE",
            args: vec![sum_of("orders", "total"), ExprKind::Value(SqlVal::F64(0.0))],
        },
    ]);

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_window_call_beside_a_bare_column_is_accepted() {
    // The engine evaluates the window over the rows the statement produced, so
    // the call names no grouping and does not collapse the relation.
    let ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        ExprKind::Window {
            kind: Box::new(WindowExprKind::Expr(sum_of("orders", "total"))),
            partition_by: vec![ExprKind::Column {
                table: "orders",
                name: "user_id",
            }],
            order_by: Vec::new(),
            frame: None,
        },
    ]);

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_reading_a_grouped_column_is_accepted_at_the_ast_level() {
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_matches_a_grouped_column_through_the_other_node_shape() {
    // The grouping records the schema-marker shape while the predicate reads the
    // alias-qualified shape of the same column of the same relation.
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::QualifiedColumn {
            table: "orders",
            column: "user_id",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(7))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_over_an_aggregate_is_accepted_at_the_ast_level() {
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(count_star()),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_having_predicate_reading_an_ungrouped_ast_column_is_rejected() {
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }];
    ast.having = Some(ExprKind::Binary {
        op: BinaryOp::And,
        lhs: Box::new(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
        }),
        rhs: Box::new(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::F64(100.0))),
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn build_error_display_unresolved_column() {
    let err = BuildError::UnresolvedColumn {
        table: "orders",
        column: "total",
    };
    let msg = err.to_string();
    assert!(msg.contains("`orders.total`"));
    assert!(msg.contains("addresses no relation"));
    assert!(msg.contains("`FROM`"));
}

/// Builds a statement projecting `projections` without introducing any relation.
fn relation_free_statement(projections: Vec<ExprKind>) -> SelectAst {
    let mut ast = empty_ast();
    ast.projections = projections;
    ast
}

#[test]
fn a_plain_column_projected_by_a_statement_without_relations_is_rejected() {
    // The statement renders no `FROM` clause, so the bare table name of the column
    // it projects names a relation nothing declares. The builder requires the table
    // to be in scope, so the statement is assembled past the type system.
    let ast = relation_free_statement(vec![ExprKind::Column {
        table: "orders",
        name: "total",
    }]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_read_by_a_filter_without_relations_is_rejected() {
    let mut ast = relation_free_statement(vec![ExprKind::Value(SqlVal::I64(1))]);
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::F64(0.0))),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_projected_beside_the_relation_it_names_is_accepted() {
    // The statement introduces the relation the reference names, so the renderer
    // qualifies the column against it.
    let ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "total",
    }]);

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_nested_statement_reaches_a_plain_column_of_an_enclosing_relation() {
    // The nested statement introduces no relation of its own, so the bare
    // reference resolves by correlation through the relations in scope where it is
    // written.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    assert_eq!(
        reject_unresolved_qualifiers(&scalar_subquery(inner), &[TableRef::plain("orders")], &[]),
        Ok(())
    );
}

#[test]
fn a_nested_statement_reading_a_plain_column_of_no_relation_is_rejected() {
    // Nothing encloses the nested statement with a relation and it introduces none
    // itself, so the bare reference names a relation the rendered statement never
    // declares.
    let mut inner = empty_ast();
    inner.projections.push(ExprKind::Column {
        table: "orders",
        name: "total",
    });

    assert_eq!(
        reject_unresolved_qualifiers(&scalar_subquery(inner), &[], &[]),
        Err(BuildError::UnresolvedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_naming_a_relation_the_statement_does_not_address_is_rejected() {
    // The statement reads `orders` while the reference names `recent`: no relation
    // in scope carries that name, so the rendered qualifier would address a table
    // the engine cannot find.
    let mut ast = relation_statement("orders", "id");
    ast.projections.push(ExprKind::Column {
        table: "recent",
        name: "total",
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "recent",
            column: "total",
        })
    );
}

#[test]
fn a_plain_column_naming_the_alias_of_an_aliased_relation_is_accepted() {
    // The statement declares the relation as `"orders" AS "o"`, so a reference
    // naming `o` reaches it through the alias the renderer writes verbatim.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::aliased("orders", "o"));
    ast.projections.push(ExprKind::Column {
        table: "o",
        name: "id",
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_plain_column_naming_the_table_behind_an_alias_is_accepted() {
    // The renderer substitutes the alias of an aliased occurrence for the table
    // name the reference carries, so the statement stays resolvable.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::aliased("orders", "o"));
    ast.projections.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_plain_column_naming_a_derived_relation_is_accepted() {
    // A derived table joins the `FROM` clause under the name it is exposed as, so
    // a reference naming it resolves like a reference to a stored relation.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("totals"));
    ast.projections.push(ExprKind::Column {
        table: "totals",
        name: "total",
    });
    ast.derived.push(derived("totals"));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_nested_plain_column_correlating_to_the_enclosing_statement_is_accepted() {
    // The nested statement introduces `orders` itself and reads `users` from the
    // statement enclosing it, which is exactly what correlation provides.
    let mut inner = relation_statement("orders", "id");
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "user_id",
        }),
        rhs: Box::new(ExprKind::Column {
            table: "users",
            name: "id",
        }),
    });

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(inner));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_nested_plain_column_naming_no_relation_in_scope_is_rejected() {
    // Neither the nested statement nor the one enclosing it introduces `ghosts`,
    // so the reference resolves to nothing at any level of the statement tree.
    let mut inner = relation_statement("orders", "id");
    inner.filter = Some(ExprKind::Column {
        table: "ghosts",
        name: "id",
    });

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(inner));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedColumn {
            table: "ghosts",
            column: "id",
        })
    );
}

#[test]
fn a_distinct_ordering_matches_a_projected_aggregate() {
    // The ordering criterion repeats the aggregate the projection list
    // exposes, so the DISTINCT rule comparing assembled expressions accepts
    // it: the statement deduplicates rows it sorts by the counted value.
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("orders"));
    ast.distinct = true;
    ast.projections.push(count_star());
    ast.order_by.push(OrderCriterion::new(
        count_star(),
        OrderDirection::Desc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_grouped_statement_renders_having_ordering_limit_and_offset() {
    // The grouped query keeps its per-group predicates, deterministic
    // ordering, and pagination after `.group_by(...)` transitioned the
    // typestate, so the tail clauses render in their fixed SQL positions.
    let built = SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .group_by(OrderUserId)
        .select((OrderUserId, Expr::count_all()))
        .having(Expr::count_all().gt(1i64))
        .order_by_desc(OrderUserId)
        .limit(5)
        .offset(10)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING (COUNT(*) > $1) ORDER BY \"user_id\" DESC LIMIT 5 OFFSET 10"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}
