//! Aggregate detection and the rules of an aggregate trunk.
//!
//! A statement projecting an aggregate without a `GROUP BY` clause collapses its
//! relation into one row, so a bare column read beside that aggregate names a
//! value the statement never reports. The tests below pin the detection helpers
//! down beside the acceptances and the exact rejections of the pass.

use super::super::{MockDialect, Order, OrderUserId};
use super::{count_star, empty_ast, orders_statement, sum_of};
use crate::expr::{
    BinaryOp, CaseBranch, Expr, ExprKind, OrderCriterion, OrderDirection, SqlVal, Subquery,
    SubqueryOp,
};
use crate::query::validate::{collect_bare_columns, contains_bare_column, has_aggregate};
use crate::query::*;

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
    // reference computed beside it is collected, in the canonical form both node
    // shapes resolve to.
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
    // The aggregate belongs to the nested statement and collapses the rows of its
    // own `FROM` scope, so it does not make the projection it is embedded into an
    // aggregate.
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
    // A reference built through a table alias names a column of a relation like the
    // schema marker does, so it is reported through the canonical pair.
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
fn an_aggregate_trunk_ordering_by_a_bare_column_is_rejected_before_rendering() {
    // An aggregate trunk collapses its relation into one row, so an ordering
    // criterion reading a stored column names a value that row does not carry.
    let mut ast = orders_statement(vec![count_star()]);
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn an_aggregate_trunk_deduplicating_by_a_bare_column_is_rejected_before_rendering() {
    // `DISTINCT ON` runs over the collapsed row as well. The first statement
    // repeats the clause in the leading ordering criterion, which isolates the
    // grouping rule from the prefix alignment; the second drops that criterion, so
    // the deduplication is the only clause reading the column, and it spells the
    // reference through the alias-qualified shape to exercise the comparison.
    let mut prefixed = orders_statement(vec![count_star()]);
    prefixed.distinct_on.push(ExprKind::Column {
        table: "orders",
        name: "user_id",
    });
    prefixed.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(
        prefixed.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "user_id",
        })
    );

    let mut deduplicated = orders_statement(vec![count_star()]);
    deduplicated.distinct_on.push(ExprKind::QualifiedColumn {
        table: "orders",
        column: "user_id",
    });

    assert_eq!(
        deduplicated.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "user_id",
        })
    );
}

#[test]
fn an_aggregate_trunk_orders_by_an_aggregate_and_a_constant() {
    // An ordering criterion computed from aggregates and constants reads one value
    // per collapsed row, so the trunk keeps it and renders the tail as written.
    let mut ast = orders_statement(vec![count_star()]);
    ast.order_by
        .push(OrderCriterion::new(count_star(), OrderDirection::Desc));
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["1".to_owned()],
            params: Vec::new(),
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("the statement renders");

    assert_eq!(
        sql,
        "SELECT COUNT(*) FROM \"orders\" ORDER BY COUNT(*) DESC, 1 ASC"
    );
    assert!(params.is_empty());
}

#[test]
fn an_aggregate_trunk_deduplicates_by_an_aggregate() {
    // The deduplication reads a value the aggregate computes over the collapsed
    // rows, so the clause stays valid and its prefix alignment holds.
    let mut ast = orders_statement(vec![count_star()]);
    ast.distinct_on.push(count_star());
    ast.order_by
        .push(OrderCriterion::new(count_star(), OrderDirection::Asc));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_per_row_trunk_still_orders_by_a_bare_column() {
    // A trunk without an aggregate produces one row per source row, so a stored
    // column is exactly what its ordering criterion may read.
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "id",
    }]);
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
        OrderDirection::Desc,
    ));

    assert_eq!(ast.validate(), Ok(()));
}
