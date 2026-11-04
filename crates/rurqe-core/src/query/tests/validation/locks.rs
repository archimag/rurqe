//! Sealing of the row-level lock clause.
//!
//! A lock claims the rows a statement read for the transaction that runs it, so it
//! describes a statement of its own. The builder keeps it there through the
//! `Unlocked` bound and the lock typestates, while the statement data can still
//! describe the pairs the engine refuses — and, for a nested statement, the pair the
//! conversions refuse. The rules below run on that data during `.build()`.

use super::super::{MockDialect, Order, OrderId};
use super::dml::TrunkReturning;
use super::{empty_ast, exists_subquery, relation_statement, scalar_subquery, UpsertDialect};
use crate::expr::{
    CteAst, DerivedAst, ExprKind, JoinAst, JoinKind, LockClause, LockStrength, OrderCriterion,
    OrderDirection, SelectAst, SetOpAst, SetOpKind,
};
use crate::query::*;
use crate::schema::TableRef;

/// Builds a statement over `orders` requesting a row-level lock.
fn locked_statement() -> SelectAst {
    let mut ast = relation_statement("orders", "id");
    ast.lock = Some(LockClause::new(LockStrength::Update));
    ast
}

#[test]
fn a_distinct_statement_requesting_a_row_lock_is_rejected_before_rendering() {
    // `SELECT DISTINCT ... FOR UPDATE` is refused: the lock claims the rows the
    // statement read while the deduplication keeps one row per group. The builder
    // keeps the pair out of reach — `distinct` moves the query out of `NoLock` — so
    // the clauses are assembled on the statement data.
    let mut ast = relation_statement("orders", "id");
    ast.distinct = true;
    ast.lock = Some(LockClause::new(LockStrength::Share));

    assert_eq!(ast.validate(), Err(BuildError::DistinctWithLock));
}

#[test]
fn a_distinct_on_statement_requesting_a_row_lock_is_rejected_before_rendering() {
    // The builder cannot reach this statement — `distinct_on` moves the query out of
    // `NoLock` — so the clause pair is assembled on the statement data, which is what
    // a statement embedded as plain AST still describes.
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
fn a_distinct_on_statement_reports_its_own_lock_diagnostic() {
    // A statement deduplicating on expressions keeps the diagnostic it has always
    // reported, so the two flavours stay distinguishable.
    let mut ast = relation_statement("orders", "id");
    ast.distinct_on.push(ExprKind::Column {
        table: "orders",
        name: "id",
    });
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
fn a_locked_statement_nested_in_a_predicate_is_rejected_before_rendering() {
    // The nested statement supplies rows to the `EXISTS` predicate of the statement
    // around it, so the lock it requests belongs to no statement of its own.
    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(locked_statement()));

    assert_eq!(ast.validate(), Err(BuildError::NestedLock));
}

#[test]
fn a_locked_common_table_expression_is_rejected_before_rendering() {
    let mut ast = relation_statement("users", "id");
    ast.ctes.push(CteAst {
        name: "pending",
        query: locked_statement(),
    });

    assert_eq!(ast.validate(), Err(BuildError::NestedLock));
}

#[test]
fn a_locked_derived_table_is_rejected_before_rendering() {
    let mut ast = empty_ast();
    ast.from = Some(TableRef::plain("pending"));
    ast.projections.push(ExprKind::Column {
        table: "pending",
        name: "id",
    });
    ast.derived.push(DerivedAst {
        name: "pending",
        query: locked_statement(),
    });

    assert_eq!(ast.validate(), Err(BuildError::NestedLock));
}

#[test]
fn a_locked_lateral_subquery_is_rejected_before_rendering() {
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::CrossLateral,
        table: TableRef::plain("recent"),
        lateral: Some(locked_statement()),
        on: None,
    });
    ast.projections.push(ExprKind::Column {
        table: "recent",
        name: "id",
    });

    assert_eq!(ast.validate(), Err(BuildError::NestedLock));
}

#[test]
fn a_locked_set_operation_operand_is_rejected_before_rendering() {
    let mut ast = relation_statement("users", "id");
    ast.set_operations.push(SetOpAst {
        kind: SetOpKind::UnionAll,
        query: locked_statement(),
    });

    assert_eq!(ast.validate(), Err(BuildError::NestedLock));
}

#[test]
fn a_lock_nested_two_statements_deep_is_rejected_as_well() {
    let mut middle = relation_statement("orders", "id");
    middle.projections = vec![scalar_subquery(locked_statement())];

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(middle));

    assert_eq!(ast.validate(), Err(BuildError::NestedLock));
}

#[test]
fn a_lock_nested_in_a_returning_projection_is_rejected() {
    use crate::dml::InsertQuery;

    // The `RETURNING` clause belongs to the statement, so a nested statement it
    // embeds is rejected by the same rule that audits a projection of a `SELECT`.
    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set(1i64))
        .returning(TrunkReturning(scalar_subquery(locked_statement())))
        .build();

    assert_eq!(built, Err(BuildError::NestedLock));
}

#[test]
fn a_locked_statement_at_the_root_of_a_build_chain_stays_valid() {
    // The lock belongs to the statement the application executes itself, so the
    // top-level statement keeps its clause and renders it unchanged.
    let mut ast = locked_statement();
    ast.order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "id",
        },
        OrderDirection::Asc,
    ));

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("a locking statement of its own stays renderable");

    // The clause closes the statement, so the lock renders after every other one.
    assert_eq!(
        sql,
        "SELECT \"id\" FROM \"orders\" ORDER BY \"id\" ASC FOR UPDATE"
    );
    assert!(params.is_empty());
}

#[test]
fn a_nested_statement_requesting_no_lock_stays_valid() {
    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(relation_statement("orders", "user_id")));

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn build_error_display_distinct_with_lock() {
    let msg = BuildError::DistinctWithLock.to_string();
    assert!(msg.contains("DISTINCT"));
    assert!(msg.contains("row-level lock"));
    assert!(msg.contains("FOR UPDATE"));
}

#[test]
fn build_error_display_nested_lock() {
    let msg = BuildError::NestedLock.to_string();
    assert!(msg.contains("nested statement"));
    assert!(msg.contains("row-level lock"));
    assert!(msg.contains("derived table"));
}

#[test]
fn the_lock_wait_modifiers_cannot_coexist_on_one_clause() {
    // `NOWAIT` and `SKIP LOCKED` describe two behaviours a statement may not request
    // at once. The clause records the wait behaviour as one value, so applying both
    // modifiers keeps the last one: the combination the engine refuses stays
    // unrepresentable instead of being rejected after the fact.
    let mut ast = relation_statement("orders", "id");
    ast.lock = Some(LockClause::new(LockStrength::Update).nowait().skip_locked());

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("a locking statement of its own stays renderable");

    assert!(sql.ends_with("FOR UPDATE SKIP LOCKED"));
    assert!(!sql.contains("NOWAIT"));
}
