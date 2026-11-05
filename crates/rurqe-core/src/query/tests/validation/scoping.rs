//! Relation namespace of a statement embedded as a relation.
//!
//! A derived table and an operand of a set operation are validated against the
//! relations they declare themselves, so a reference they write reaches no relation
//! of the statement embedding them. A lateral subquery is the exception: it is
//! evaluated where it is written, so it reads the relations of the statement joining
//! it. The tests below pin both sides down.

use super::{empty_ast, relation_statement};
use crate::expr::{
    BinaryOp, DerivedAst, ExprKind, JoinAst, JoinKind, SelectAst, SetOpAst, SetOpKind,
};
use crate::query::*;
use crate::schema::TableRef;

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
        query: super::qualified_body("users"),
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
        query: super::qualified_body("users"),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::UnresolvedQualifier { name: "users" })
    );
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
