//! Tests for qualifying window expressions that were freed from their scope.

use super::*;

#[test]
fn without_scope_qualifies_every_operand_of_a_window_expression() {
    // A window specification names its columns in three places: the operand of the
    // function, the partitioning, and the ordering. All three stay ordinary
    // operands of the statement projecting the window, so a tree freed from its
    // scope has to spell each of them out.
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::Lag {
            column: Expr::column::<UserId>().into_kind(),
            offset: Some(2),
        }),
        partition_by: vec![Expr::column::<UserActive>().into_kind()],
        order_by: vec![OrderCriterion::new(
            Expr::column::<UserName>().into_kind(),
            OrderDirection::Desc,
        )
        .nulls(NullsOrder::Last)],
        frame: None,
    };

    let unscoped: Expr<i64, Unscoped> = Expr::from_kind(window).without_scope();

    let ExprKind::Window {
        kind,
        partition_by,
        order_by,
        frame,
    } = unscoped.kind()
    else {
        panic!("expected a window expression, got {:?}", unscoped.kind());
    };

    assert_eq!(
        kind.as_ref(),
        &WindowExprKind::Lag {
            column: ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            },
            offset: Some(2),
        }
    );
    assert_eq!(
        partition_by,
        &[ExprKind::QualifiedColumn {
            table: "users",
            column: "active",
        }]
    );
    assert_eq!(
        order_by,
        &[OrderCriterion::new(
            ExprKind::QualifiedColumn {
                table: "users",
                column: "name",
            },
            OrderDirection::Desc,
        )
        .nulls(NullsOrder::Last)]
    );
    // The frame counts rows or values and names no column, so it is kept as is.
    assert!(frame.is_none());
}

#[test]
fn without_scope_qualifies_the_expression_a_window_evaluates() {
    // The expression a window evaluates is an operand of the enclosing statement as
    // well, so an aggregate evaluated over the window has to name the table of the
    // column it reads.
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::Expr(
            Expr::<i64>::count_distinct::<UserId>().into_kind(),
        )),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        frame: None,
    };

    let unscoped: Expr<i64, Unscoped> = Expr::from_kind(window).without_scope();

    let ExprKind::Window { kind, .. } = unscoped.kind() else {
        panic!("expected a window expression, got {:?}", unscoped.kind());
    };
    let WindowExprKind::Expr(inner) = kind.as_ref() else {
        panic!("expected the aggregate to stay the operand of the window");
    };

    assert_eq!(
        inner,
        &ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: true,
            arg: Some(Box::new(ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            })),
            filter: None,
        }
    );
}

#[test]
fn a_window_freed_from_its_scope_names_its_table_in_a_single_relation_statement() {
    // The statement the expression is rendered into addresses `users` alone, which
    // is exactly the shape where a bare reference stays unqualified: without the
    // rewrite the window would read the relation the correlated statement selects
    // from under that same name instead of the one it was built over.
    let window = ExprKind::Window {
        kind: Box::new(WindowExprKind::Lead {
            column: Expr::column::<UserId>().into_kind(),
            offset: None,
        }),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        frame: None,
    };

    let unscoped: Expr<i64, Unscoped> = Expr::from_kind(window).without_scope();

    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<MockDialect>(
        &mut sql,
        &mut params,
        unscoped.kind(),
        &[crate::schema::TableRef::plain("users")],
    )
    .expect("a valid window expression");

    assert_eq!(sql, "LEAD(\"users\".\"id\") OVER ()");
    assert!(params.is_empty());
}
