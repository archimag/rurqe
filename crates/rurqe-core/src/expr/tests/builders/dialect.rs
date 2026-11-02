//! Tests for dialect operators and quantified comparisons.

use super::*;

#[test]
fn dialect_operators_inherit_the_scope_of_their_operands() {
    // A bound right operand references no table, so the operator keeps exactly
    // the scope of the column it is applied to.
    let matched: Expr<bool, User> =
        Expr::column::<UserName>().dialect_op::<String, bool, _>("~", "^alice");

    assert_eq!(
        matched.kind(),
        &ExprKind::DialectOp {
            op: "~",
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "name",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::Text("^alice".to_owned()))),
        }
    );

    let (sql, params) = render(matched.kind());
    assert_eq!(sql, "(\"name\" ~ ?)");
    assert_eq!(params, vec![SqlVal::Text("^alice".to_owned())]);

    // A right operand reading a column of another table extends the scope with
    // that table, exactly as comparing the two columns does.
    let compared: Expr<i64, (User, Order)> =
        Expr::column::<UserId>().dialect_op::<i64, i64, _>("+", Expr::column::<OrderQty>());

    assert_eq!(
        compared.kind(),
        &ExprKind::DialectOp {
            op: "+",
            lhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "qty",
            }),
        }
    );
}

#[test]
fn dialect_operators_keep_the_dialect_marker_of_their_operands() {
    // The dialect-pinned operands are assembled with the crate-internal
    // constructor: the escape hatch builds scope-free constants only, while the
    // scope of a column reference comes from its column marker. The test
    // exercises how the dialect marker survives the operators below.
    let pinned: Expr<i64, User, NonAggregate, MockDialect> = Expr::new(ExprKind::Column {
        table: "users",
        name: "id",
    });
    let pinned_rhs: Expr<i64, User, NonAggregate, MockDialect> = Expr::new(ExprKind::Column {
        table: "users",
        name: "id",
    });

    // A dialect-neutral right operand adds nothing to the marker of the left
    // one, so the operator still requires the dialect its left operand does.
    let appended: Expr<i64, User, NonAggregate, MockDialect> =
        pinned.dialect_op::<i64, i64, _>("%", 2i64);
    assert!(matches!(
        appended.kind(),
        ExprKind::DialectOp { op: "%", .. }
    ));

    // A dialect-neutral left operand keeps the marker of its right-hand side.
    let prepended: Expr<i64, User, NonAggregate, MockDialect> =
        Expr::<i64>::literal(2i64).dialect_op::<i64, i64, _>("%", pinned_rhs);
    assert!(matches!(
        prepended.kind(),
        ExprKind::DialectOp { op: "%", .. }
    ));
}

#[test]
fn quantified_comparisons_wrap_the_quantifier_around_their_operand() {
    // The receiver is the quantified side, so the searched value lands on the
    // left of the comparison and the quantifier wraps the receiver.
    let matched: Expr<bool, Order> =
        Expr::column::<OrderQty>().dialect_quantified::<i64, _>("ANY", 7i64);

    assert_eq!(
        matched.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::Value(SqlVal::I64(7))),
            rhs: Box::new(ExprKind::Function {
                name: "ANY",
                args: vec![ExprKind::Column {
                    table: "orders",
                    name: "qty",
                }],
            }),
        }
    );

    let (sql, params) = render(matched.kind());
    assert_eq!(sql, "(? = ANY(\"qty\"))");
    assert_eq!(params, vec![SqlVal::I64(7)]);
}

#[test]
fn quantified_comparisons_inherit_the_scopes_of_both_operands() {
    // A neutral right operand contributes no table, so the scope is the table of
    // the quantified column.
    let over_a_column: Expr<bool, User> =
        Expr::column::<UserName>().dialect_quantified::<String, _>("ANY", "^alice".to_owned());
    assert!(matches!(
        over_a_column.kind(),
        ExprKind::Binary {
            op: BinaryOp::Eq,
            ..
        }
    ));

    // A left operand reading a column extends the merged scope as well, so the
    // comparison references both tables.
    let over_two_tables: Expr<bool, (Order, User)> = Expr::column::<OrderQty>()
        .dialect_quantified::<String, _>("ANY", Expr::column::<UserName>());
    assert!(matches!(
        over_two_tables.kind(),
        ExprKind::Binary {
            op: BinaryOp::Eq,
            ..
        }
    ));
}
