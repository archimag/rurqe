//! Placement rules of an aggregate expression.
//!
//! A clause the engine evaluates row by row — the `WHERE` predicate, the `GROUP BY`
//! entries, the `ON` condition of a join, the `SET` assignment of an update, and the
//! conflict predicates of an upsert — is resolved before the rows are collapsed into
//! groups, so an aggregate written there names no value the clause can read. The
//! tests below pin the rejection down beside the nested statement an aggregate is
//! allowed to belong to.

use super::super::{Order, OrderId, OrderTotal};
use super::{
    count_star, exists_subquery, orders_statement, relation_statement, scalar_subquery, sum_of,
    UpsertDialect,
};
use crate::expr::{BinaryOp, Expr, ExprKind, JoinAst, JoinKind, SqlVal, WindowExprKind};
use crate::query::*;
use crate::schema::TableRef;

#[test]
fn an_aggregate_in_a_where_predicate_is_rejected_before_rendering() {
    // The predicate decides which rows take part in the statement, so the engine
    // evaluates it before the rows are collapsed into groups. The builder keeps it
    // out at the call site through its aggregation marker, so the statement is
    // assembled from its AST.
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "id",
    }]);
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(count_star()),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::Where,
            aggregate: "COUNT",
        })
    );
}

#[test]
fn an_aggregate_in_a_group_by_clause_is_rejected_before_rendering() {
    // The grouping decides how the rows are collapsed, so it cannot read a value that
    // only exists once they are collapsed.
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "user_id",
    }]);
    ast.group_by = vec![sum_of("orders", "total")];

    assert_eq!(
        ast.validate(),
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::GroupBy,
            aggregate: "SUM",
        })
    );
}

#[test]
fn an_aggregate_in_a_join_on_condition_is_rejected_before_rendering() {
    // The `ON` condition decides which pairs of rows the join produces, so it is
    // evaluated per row of either side.
    let mut ast = relation_statement("users", "id");
    ast.joins.push(JoinAst {
        kind: JoinKind::Inner,
        table: TableRef::plain("orders"),
        lateral: None,
        on: Some(ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(count_star()),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
        }),
    });

    assert_eq!(
        ast.validate(),
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::JoinOn,
            aggregate: "COUNT",
        })
    );
}

#[test]
fn a_where_predicate_comparing_against_an_aggregate_subquery_is_accepted() {
    // The aggregate belongs to the nested statement, which projects nothing beside
    // it, while the enclosing predicate compares a per-row column against the one
    // value the subquery computes: the rule stops at the statement boundary.
    let mut inner = relation_statement("orders", "id");
    inner.projections = vec![sum_of("orders", "total")];

    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "id",
    }]);
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Column {
            table: "orders",
            name: "total",
        }),
        rhs: Box::new(scalar_subquery(inner)),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_where_predicate_reading_a_window_call_is_not_rejected() {
    // A window call is evaluated over the rows the statement produced rather than
    // collapsing them, so the placement rule does not report it here.
    let mut ast = orders_statement(vec![ExprKind::Column {
        table: "orders",
        name: "id",
    }]);
    ast.filter = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(ExprKind::Window {
            kind: Box::new(WindowExprKind::RowNumber),
            partition_by: Vec::new(),
            order_by: Vec::new(),
            frame: None,
        }),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
    });

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn an_aggregate_assigned_to_a_column_is_rejected_before_rendering() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The `SET` clause computes one value per affected row, so an aggregate written
    // into the assignment describes a value no single row carries.
    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set_expr(
            Expr::<f64>::from_kind(sum_of("orders", "total")).without_scope(),
        ))
        .allow_all()
        .build();

    assert_eq!(
        built,
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::Assignment,
            aggregate: "SUM",
        })
    );
}

#[test]
fn an_aggregate_in_a_conflict_target_predicate_is_rejected_before_rendering() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let predicate = Expr::<bool>::from_kind(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(sum_of("orders", "id")),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
    });

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set(1i64))
        .on_conflict((OrderId,))
        .target_where(predicate)
        .do_nothing()
        .build();

    assert_eq!(
        built,
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::ConflictTarget,
            aggregate: "SUM",
        })
    );
}

#[test]
fn an_aggregate_in_a_conflict_update_predicate_is_rejected_before_rendering() {
    use crate::dml::{ColumnAssign, InsertQuery};

    let predicate = Expr::<bool>::from_kind(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(count_star()),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
    });

    let built = InsertQuery::<UpsertDialect, Order>::new()
        .values(OrderId::set(1i64))
        .on_conflict((OrderId,))
        .update_where(predicate)
        .do_update(OrderTotal::set(1.0f64))
        .build();

    assert_eq!(
        built,
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::ConflictUpdate,
            aggregate: "COUNT",
        })
    );
}

#[test]
fn an_aggregate_in_a_mutation_filter_is_rejected_before_rendering() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The `WHERE` predicate of a mutation decides which rows take part, so the
    // engine evaluates it before any aggregate could collapse them. The call site
    // marks the predicate as per-row, so the aggregate is smuggled into that marker
    // by the tree alone — which is exactly what tier 2 has to catch.
    let predicate = Expr::column::<OrderId>().gt(Expr::<i64>::from_kind(sum_of("orders", "total")));

    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set(2.5f64))
        .where_(predicate)
        .build();

    assert_eq!(
        built,
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::Where,
            aggregate: "SUM",
        })
    );
}

#[test]
fn an_aggregate_in_a_deletion_filter_is_rejected_before_rendering() {
    use crate::dml::DeleteQuery;

    let predicate = Expr::column::<OrderId>().gt(Expr::<i64>::from_kind(count_star()));

    let built = DeleteQuery::<UpsertDialect, Order>::new()
        .where_(predicate)
        .build();

    assert_eq!(
        built,
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::Where,
            aggregate: "COUNT",
        })
    );
}

#[test]
fn build_error_display_aggregate_in_row_clause() {
    let err = BuildError::AggregateInRowClause {
        clause: RowClause::Where,
        aggregate: "COUNT",
    };
    let msg = err.to_string();
    assert!(msg.contains("aggregate function `COUNT`"));
    assert!(msg.contains("the `WHERE` predicate"));
    assert!(msg.contains("row by row"));
    assert!(msg.contains("`HAVING`"));
}

#[test]
fn an_aggregate_in_the_filter_of_a_nested_statement_is_rejected() {
    // The predicate of the nested statement decides which of its rows take part, so
    // the engine evaluates it before those rows could be collapsed.
    let mut inner = relation_statement("orders", "id");
    inner.filter = Some(ExprKind::Binary {
        op: BinaryOp::Gt,
        lhs: Box::new(count_star()),
        rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
    });

    let mut ast = relation_statement("users", "id");
    ast.filter = Some(exists_subquery(inner));

    assert_eq!(
        ast.validate(),
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::Where,
            aggregate: "COUNT",
        })
    );
}

#[test]
fn an_aggregate_in_the_grouping_of_a_nested_statement_is_rejected() {
    // The grouping of the nested statement decides how its rows are collapsed, so it
    // cannot itself read a value that only exists once they are collapsed.
    let mut inner = relation_statement("orders", "user_id");
    inner.projections = vec![ExprKind::Value(SqlVal::I64(1))];
    inner.group_by = vec![sum_of("orders", "total")];

    let mut ast = relation_statement("users", "id");
    ast.projections.push(scalar_subquery(inner));

    assert_eq!(
        ast.validate(),
        Err(BuildError::AggregateInRowClause {
            clause: RowClause::GroupBy,
            aggregate: "SUM",
        })
    );
}

#[test]
fn an_update_assignment_reading_a_scalar_subquery_with_an_aggregate_is_accepted() {
    use crate::dml::{ColumnAssign, UpdateQuery};

    // The aggregate belongs to the nested statement, which collapses its own rows
    // into the one value the assignment reads, so the assigned expression stays a
    // per-row value: the placement rule stops at the statement boundary.
    let mut inner = relation_statement("orders", "total");
    inner.projections = vec![sum_of("orders", "total")];

    let built = UpdateQuery::<UpsertDialect, Order>::new()
        .set(OrderTotal::set_expr(
            Expr::<f64>::from_kind(scalar_subquery(inner)).without_scope(),
        ))
        .allow_all()
        .build()
        .expect("a scalar subquery aggregates its own scope");

    assert!(built.sql.contains("SELECT SUM(\"total\") FROM \"orders\""));
    assert!(built.params.is_empty());
}
