//! Grouping coverage and operand grammar of a window expression.
//!
//! A window call is evaluated over the rows the statement produced, so it collapses
//! nothing itself and never turns a per-row trunk into an aggregate trunk. Its
//! operand is read from those produced rows, though, and the grouping decides which
//! values they carry, so the walk descends into the operand and stops again at an
//! aggregate nested inside it.
//!
//! The operand is additionally a function application: the grammar attaches `OVER`
//! to a call and to nothing else, so the shapes that name a value rather than a call
//! are rejected while the statement is built, and the tests below pin both the
//! rejection and the accepted call shapes down beside the grouping cases.

use super::super::{MockDialect, Order, OrderTotal, OrderUserId};
use super::{count_star, orders_statement, orders_user_id, scalar_subquery, sum_of};
use crate::expr::{
    AggregateFunc, BinaryOp, Expr, ExprKind, OrderCriterion, OrderDirection, SqlVal,
    WindowExprKind,
};
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
            order_by: Vec::new(),
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
        order_by: Vec::new(),
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

#[test]
fn a_grouped_statement_rejects_a_window_partitioned_by_an_ungrouped_column() {
    // The partition of a window is read from the rows the statement produced, so
    // every column it names has to survive the grouping exactly like a projected
    // one: the grouping dropped the per-row value of `total`, which leaves the
    // window without the value it would partition by.
    let mut ast = orders_statement(vec![
        orders_user_id(),
        window_over(
            count_star(),
            vec![ExprKind::Column {
                table: "orders",
                name: "total",
            }],
        ),
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
fn a_window_over_a_bound_value_is_rejected_before_rendering() {
    // An `OVER` clause attaches only to a function application, so a bound value
    // cannot carry the window however the grouping is written: the statement would
    // render `$1 OVER (...)`.
    let mut ast = orders_statement(vec![
        orders_user_id(),
        window_over(ExprKind::Value(SqlVal::I64(1)), vec![orders_user_id()]),
    ]);
    ast.group_by = vec![orders_user_id()];

    assert_eq!(
        ast.validate(),
        Err(BuildError::WindowOperandNotCall {
            operand: "a bound value",
        })
    );
}

#[test]
fn a_window_over_a_column_is_rejected_before_rendering() {
    // A column reference names a value rather than a call: the statement would
    // render `"total" OVER (...)`, which no engine parses.
    let ast = orders_statement(vec![
        orders_user_id(),
        window_over(
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
            vec![orders_user_id()],
        ),
    ]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::WindowOperandNotCall {
            operand: "a column reference",
        })
    );
}

#[test]
fn a_window_over_an_operator_expression_is_rejected_before_rendering() {
    // An arithmetic combination is not a function application either, so the operand
    // has to aggregate the rows or be wrapped into a call before the clause attaches.
    let ast = orders_statement(vec![window_over(
        ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(orders_user_id()),
            rhs: Box::new(orders_user_id()),
        },
        Vec::new(),
    )]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::WindowOperandNotCall {
            operand: "an operator expression",
        })
    );
}

#[test]
fn a_window_over_a_subquery_is_rejected_before_rendering() {
    // A scalar subquery supplies a value as well, so the window clause cannot be
    // attached to it.
    let inner = orders_statement(vec![orders_user_id()]);
    let ast = orders_statement(vec![window_over(scalar_subquery(inner), Vec::new())]);

    assert_eq!(
        ast.validate(),
        Err(BuildError::WindowOperandNotCall {
            operand: "a subquery",
        })
    );
}

#[test]
fn a_grouped_statement_accepts_a_window_over_a_function_call() {
    // A named function call is a function application, so the clause attaches to it;
    // the column it reads is grouped and therefore survives the collapse.
    let mut ast = orders_statement(vec![
        orders_user_id(),
        window_over(
            ExprKind::Function {
                name: "FIRST_VALUE",
                args: vec![orders_user_id()],
            },
            vec![orders_user_id()],
        ),
    ]);
    ast.group_by = vec![orders_user_id()];

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("a call-shaped window renders");

    assert_eq!(
        sql,
        "SELECT \"user_id\", FIRST_VALUE(\"user_id\") OVER (PARTITION BY \"user_id\") \
         FROM \"orders\" GROUP BY \"user_id\""
    );
    assert!(params.is_empty());
}

#[test]
fn a_window_over_a_raw_fragment_is_accepted() {
    // A raw fragment is trusted to spell the call, which is how a window function the
    // typed surface does not model is written.
    let ast = orders_statement(vec![
        ExprKind::Column {
            table: "orders",
            name: "id",
        },
        window_over(
            ExprKind::Raw {
                fragments: vec!["COUNT(*)".to_owned()].into(),
                params: Vec::new(),
            },
            Vec::new(),
        ),
    ]);

    assert_eq!(ast.validate(), Ok(()));

    let mut sql = String::new();
    let mut params = Vec::new();
    ast.render::<MockDialect>(&mut sql, &mut params)
        .expect("a call-shaped window renders");

    assert_eq!(sql, "SELECT \"id\", COUNT(*) OVER () FROM \"orders\"");
    assert!(params.is_empty());
}

#[test]
fn build_error_display_window_operand_not_call() {
    let err = BuildError::WindowOperandNotCall {
        operand: "a bound value",
    };
    let msg = err.to_string();
    assert!(msg.contains("a bound value"));
    assert!(msg.contains("OVER"));
    assert!(msg.contains("function application"));
}

#[test]
fn an_aggregate_ordering_its_own_rows_by_a_bare_column_stays_aggregate() {
    // The criteria of an aggregate order the rows the call consumes, so a column
    // they read is inside the aggregate exactly like its argument.
    let ordered = ExprKind::Aggregate {
        func: AggregateFunc::ArrayAgg,
        distinct: false,
        arg: Some(Box::new(orders_user_id())),
        filter: None,
        order_by: vec![OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
            OrderDirection::Asc,
        )],
    };
    let ast = orders_statement(vec![ordered]);

    assert_eq!(ast.validate(), Ok(()));
}