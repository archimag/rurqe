//! Census rules of a raw SQL expression.
//!
//! A raw fragment list has to balance its parameters against its placeholders,
//! and the rule is stated once for the whole statement walk, so it is reached
//! from a projection, a join condition, a `HAVING` predicate, and an ordering
//! criterion alike.

use super::{empty_ast, relation_statement, scalar_subquery, unbalanced_raw, unbalanced_raw_error};
use crate::expr::{BinaryOp, ExprKind, JoinAst, JoinKind, OrderCriterion, OrderDirection, RawSqlError};
use crate::query::*;
use crate::schema::TableRef;

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
    ast.order_by
        .push(OrderCriterion::new(unbalanced_raw(), OrderDirection::Asc));

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
