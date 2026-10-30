//! The marker fold of a comparison against the pre-flight pass.
//!
//! The aggregation marker of a comparison is derived from both of its operands, so
//! a predicate comparing an aggregate with a bare column is per-row and the grouped
//! rules of the builder reject it at the call site. The pre-flight pass reaches the
//! same statements assembled past the type system and has to answer the same way —
//! rejecting the per-row reference and leaving every aggregate-only predicate
//! alone, so the two tiers never disagree and no valid statement is refused.

use super::super::MockDialect;
use super::{count_star, orders_statement, orders_user_id, sum_of};
use crate::expr::{BinaryOp, ExprKind, SelectAst, SqlVal};
use crate::query::*;

/// Builds a statement over `orders` grouped by `orders.user_id`.
fn grouped_orders(projections: Vec<ExprKind>) -> SelectAst {
    let mut ast = orders_statement(projections);
    ast.group_by = vec![orders_user_id()];
    ast
}

/// Builds `lhs <op> rhs`, the node shape a marker-folded comparison produces.
fn comparison(op: BinaryOp, lhs: ExprKind, rhs: ExprKind) -> ExprKind {
    ExprKind::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

#[test]
fn a_having_predicate_comparing_an_aggregate_with_a_bare_column_is_rejected() {
    // The type system already keeps such a predicate out of `HAVING`; the same
    // statement assembled from plain AST data is rejected by the pre-flight pass,
    // which reports the per-row reference the grouping dropped.
    let mut ast = grouped_orders(vec![orders_user_id(), count_star()]);
    ast.having = Some(comparison(
        BinaryOp::Gt,
        count_star(),
        ExprKind::Column {
            table: "orders",
            name: "total",
        },
    ));

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_projection_comparing_an_aggregate_with_a_bare_column_is_rejected() {
    let ast = grouped_orders(vec![
        orders_user_id(),
        comparison(
            BinaryOp::Gt,
            count_star(),
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
        ),
    ]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_having_predicate_comparing_an_aggregate_with_a_bound_value_renders() {
    let mut ast = grouped_orders(vec![orders_user_id(), count_star()]);
    ast.having = Some(comparison(
        BinaryOp::Gt,
        count_star(),
        ExprKind::Value(SqlVal::I64(1)),
    ));

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("the statement renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
         HAVING (COUNT(*) > $1)"
    );
    assert_eq!(params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_grouped_projection_comparing_two_aggregates_is_accepted() {
    // The comparison reads no per-row value, so the grouping covers it exactly as
    // it covers the aggregates the predicate is computed from.
    let ast = grouped_orders(vec![
        orders_user_id(),
        comparison(BinaryOp::Gt, sum_of("orders", "total"), count_star()),
    ]);

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("the statement renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", (SUM(\"total\") > COUNT(*)) FROM \"orders\" GROUP BY \"user_id\""
    );
    assert!(params.is_empty());
}

#[test]
fn a_grouped_projection_comparing_an_aggregate_with_a_bound_value_renders() {
    let ast = grouped_orders(vec![
        orders_user_id(),
        comparison(BinaryOp::Gt, count_star(), ExprKind::Value(SqlVal::I64(1))),
    ]);

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("the statement renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", (COUNT(*) > $1) FROM \"orders\" GROUP BY \"user_id\""
    );
    assert_eq!(params, vec![SqlVal::I64(1)]);
}

#[test]
fn a_having_predicate_comparing_two_aggregates_is_accepted() {
    let mut ast = grouped_orders(vec![orders_user_id(), count_star()]);
    ast.having = Some(comparison(
        BinaryOp::Gt,
        sum_of("orders", "user_id"),
        count_star(),
    ));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_grouped_projection_wrapping_an_aggregate_in_a_scalar_function_is_accepted() {
    // The marker fold keeps the call an aggregate inside the function, so the
    // projection stays valid; the pre-flight pass reads the same tree and finds no
    // bare column to reject.
    let ast = grouped_orders(vec![
        orders_user_id(),
        ExprKind::Function {
            name: "COALESCE",
            args: vec![sum_of("orders", "total"), ExprKind::Value(SqlVal::F64(0.0))],
        },
    ]);

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("the statement renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", COALESCE(SUM(\"total\"), $1) FROM \"orders\" GROUP BY \"user_id\""
    );
    assert_eq!(params, vec![SqlVal::F64(0.0)]);
}
