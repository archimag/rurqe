//! Grouping coverage of a window operand.
//!
//! A window call is evaluated over the rows the statement produced, so it collapses
//! nothing itself and never turns a per-row trunk into an aggregate trunk. Its
//! operand is read from those produced rows, though, and the grouping decides which
//! values they carry, so the walk descends into the operand and stops again at an
//! aggregate nested inside it.

use super::super::{MockDialect, Order, OrderTotal, OrderUserId};
use super::{count_star, orders_statement, orders_user_id, sum_of};
use crate::expr::{AggregateFunc, BinaryOp, Expr, ExprKind, SqlVal, WindowExprKind};
use crate::query::*;

/// Builds `operand OVER (PARTITION BY ...)` over the given partition expressions.
fn window_over(operand: ExprKind, partition_by: Vec<ExprKind>) -> ExprKind {
    ExprKind::Window {
        kind: Box::new(WindowExprKind::Expr(operand)),
        partition_by,
        order_by: Vec::new(),
        frame: None,
    }
}

/// Builds `SUM(SUM(table.name)) OVER (...)` — a window evaluated over an aggregate.
fn sum_of_sum_over(
    table: &'static str,
    name: &'static str,
    partition_by: Vec<ExprKind>,
) -> ExprKind {
    window_over(
        ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: false,
            arg: Some(Box::new(sum_of(table, name))),
            filter: None,
        },
        partition_by,
    )
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
fn a_window_call_beside_a_bare_column_is_accepted() {
    // The engine evaluates the window over the rows the statement produced, so the
    // call names no grouping and does not collapse the relation — and because the
    // trunk collapses nothing either, the operand keeps reading the column of every
    // source row.
    let ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "user_id",
        },
        window_over(
            sum_of("orders", "total"),
            vec![ExprKind::Column {
                table: "orders",
                name: "user_id",
            }],
        ),
    ]);

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("a per-row window renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", SUM(\"total\") OVER (PARTITION BY \"user_id\") FROM \"orders\""
    );
    assert!(params.is_empty());
}

#[test]
fn a_grouped_statement_projecting_a_window_over_an_ungrouped_column_is_rejected() {
    // The window reads the rows of its partition after the grouping collapsed them,
    // so the column its aggregate reads has to be covered by that grouping exactly
    // like the operand of a statement aggregate.
    let mut ast = orders_statement(vec![
        orders_user_id(),
        window_over(sum_of("orders", "total"), vec![orders_user_id()]),
    ]);
    ast.group_by = vec![orders_user_id()];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn an_aggregate_trunk_projecting_a_window_over_a_bare_column_is_rejected() {
    // An aggregate trunk collapses the relation into one row exactly like a grouping
    // over the empty set, so the window over the collapsed rows cannot read a stored
    // column the aggregate does not cover either.
    let ast = orders_statement(vec![
        count_star(),
        window_over(sum_of("orders", "total"), Vec::new()),
    ]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumnWithAggregate {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_window_filter_reading_an_ungrouped_column_is_rejected() {
    // `FILTER (WHERE ...)` restricts the rows fed to the window aggregate, and those
    // rows are the produced ones: the condition reads a column the grouping dropped
    // as well, while the aggregated argument stays grouped.
    let filtered = ExprKind::Aggregate {
        func: AggregateFunc::Sum,
        distinct: false,
        arg: Some(Box::new(orders_user_id())),
        filter: Some(Box::new(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::F64(100.0))),
        })),
    };

    let mut ast = orders_statement(vec![
        orders_user_id(),
        window_over(filtered, vec![orders_user_id()]),
    ]);
    ast.group_by = vec![orders_user_id()];

    assert_eq!(
        ast.validate(),
        Err(BuildError::UngroupedColumn {
            table: "orders",
            column: "total",
        })
    );
}

#[test]
fn a_grouped_statement_accepts_a_window_over_a_grouped_column() {
    // Every column the operand reads is grouped, so the window is evaluated over one
    // value per group and the statement stays valid.
    let mut ast = orders_statement(vec![
        orders_user_id(),
        window_over(sum_of("orders", "user_id"), vec![orders_user_id()]),
    ]);
    ast.group_by = vec![orders_user_id()];

    assert_eq!(ast.validate(), Ok(()));
}

#[test]
fn a_window_over_an_aggregate_is_accepted_in_a_grouped_statement() {
    // The inner aggregate collapses the groups before the window reads them, so the
    // grouping has no claim on `total`: the statement is valid and renders.
    let mut ast = orders_statement(vec![
        orders_user_id(),
        sum_of_sum_over("orders", "total", vec![orders_user_id()]),
    ]);
    ast.group_by = vec![orders_user_id()];

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("a window over an aggregate renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", SUM(SUM(\"total\")) OVER (PARTITION BY \"user_id\") \
         FROM \"orders\" GROUP BY \"user_id\""
    );
    assert!(params.is_empty());
}

#[test]
fn a_window_over_an_aggregate_is_accepted_beside_an_aggregate_trunk() {
    // The inner call covers the column it reads, so the aggregate trunk has nothing
    // left to reject: the collapsed row carries the aggregated value the window then
    // reads.
    let ast = orders_statement(vec![
        count_star(),
        sum_of_sum_over("orders", "total", Vec::new()),
    ]);

    assert_eq!(ast.validate(), Ok(()));
}
