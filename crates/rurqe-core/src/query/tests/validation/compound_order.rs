//! Ordering rules of a compound statement.
//!
//! `UNION`, `INTERSECT`, and `EXCEPT` produce rows of their own, so the ordering
//! that sorts them addresses output column names and 1-based positions instead
//! of relation references. The tests below pin the accepted forms beside the
//! exact diagnostics every rejected form reports.

use super::super::{User, UserName};
use super::{empty_ast, labelled_compound, scalar_subquery, UpsertDialect};
use crate::expr::{AggregateFunc, Expr, ExprKind, OrderCriterion, OrderDirection};
use crate::query::*;
use crate::schema::TableRef;

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
            fragments: vec!["1".to_owned(), " + 1".to_owned()].into(),
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
            fragments: vec!["1".to_owned()].into(),
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
fn a_compound_statement_sorts_its_output_columns_rather_than_a_relation_of_an_operand() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The criterion names the output column `label` of the projection list while the
    // node carries the table of a relation the left operand never declares. The
    // assignment embeds the statement as a scalar subquery, which is exactly the
    // shape the DML qualifier pass reaches: resolving the criterion against the
    // relation scope of the statement would reject valid SQL, so the statement is
    // accepted and rendered with the bare output column.
    let mut compound = labelled_compound();
    compound.compound_order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "label",
        },
        OrderDirection::Asc,
    ));

    let built = UpdateQuery::<UpsertDialect, User>::new()
        .set(UserName::set_expr(
            Expr::<String>::from_kind(scalar_subquery(compound)).without_scope(),
        ))
        .allow_all()
        .build()
        .expect("a compound ordering addresses the output columns of the combination");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = (SELECT \"users\".\"name\" AS \"label\" FROM \"users\" \
         UNION SELECT \"total\" FROM \"orders\" ORDER BY \"label\" ASC)"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_compound_statement_embedded_in_an_assignment_orders_by_an_output_position() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    let mut compound = labelled_compound();
    compound.compound_order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["1".to_owned()].into(),
            params: Vec::new(),
        },
        OrderDirection::Desc,
    ));

    let built = UpdateQuery::<UpsertDialect, User>::new()
        .set(UserName::set_expr(
            Expr::<String>::from_kind(scalar_subquery(compound)).without_scope(),
        ))
        .allow_all()
        .build()
        .expect("a compound ordering addresses a 1-based position of the projection list");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = (SELECT \"users\".\"name\" AS \"label\" FROM \"users\" \
         UNION SELECT \"total\" FROM \"orders\" ORDER BY 1 DESC)"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_compound_statement_ordering_by_an_unprojected_output_column_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The projection list exposes `label` alone, so an ordering naming another
    // output column is rejected by the statement rules before any SQL is rendered.
    let mut compound = labelled_compound();
    compound.compound_order_by.push(OrderCriterion::new(
        ExprKind::Column {
            table: "orders",
            name: "ghost",
        },
        OrderDirection::Asc,
    ));

    let built = UpdateQuery::<UpsertDialect, User>::new()
        .set(UserName::set_expr(
            Expr::<String>::from_kind(scalar_subquery(compound)).without_scope(),
        ))
        .allow_all()
        .build();

    assert_eq!(
        built,
        Err(BuildError::CompoundOrderColumn { column: "ghost" })
    );
}

#[test]
fn a_compound_statement_ordering_by_a_position_outside_its_arity_is_rejected() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The combination produces one output column, so a position beyond it addresses
    // nothing the combined rows carry.
    let mut compound = labelled_compound();
    compound.compound_order_by.push(OrderCriterion::new(
        ExprKind::Raw {
            fragments: vec!["2".to_owned()].into(),
            params: Vec::new(),
        },
        OrderDirection::Asc,
    ));

    let built = UpdateQuery::<UpsertDialect, User>::new()
        .set(UserName::set_expr(
            Expr::<String>::from_kind(scalar_subquery(compound)).without_scope(),
        ))
        .allow_all()
        .build();

    assert_eq!(
        built,
        Err(BuildError::CompoundOrderIndex {
            index: 2,
            columns: 1,
        })
    );
}
