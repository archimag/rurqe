//! Tests for the expression assembled from literal SQL text and operand slots.

use core::fmt::Write;

use super::*;
use crate::query::{BuildError, SelectQuery};

/// A dialect quoting identifiers with brackets and numbering its placeholders.
struct BracketDialect;

impl crate::dialect::Dialect for BracketDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('[');
        out.push_str(ident);
        out.push(']');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        let _ = write!(out, "${index}");
    }
}

#[test]
fn a_column_slot_is_rendered_with_the_quoting_of_the_dialect() {
    let lowered = Expr::<String>::fragments("lower(")
        .operand(UserName)
        .text(")")
        .end();

    assert_eq!(
        lowered.kind(),
        &ExprKind::Fragments {
            fragments: RawFragments::Owned(vec!["lower(".to_owned(), ")".to_owned()]),
            operands: vec![ExprKind::Column {
                table: "users",
                name: "name",
            }],
        }
    );

    let (sql, params) = render(lowered.kind());
    assert_eq!(sql, "lower(\"name\")");
    assert!(params.is_empty());

    let mut bracketed = String::new();
    let mut bound = Vec::new();
    render_expr::<BracketDialect>(&mut bracketed, &mut bound, lowered.kind(), &[])
        .expect("a valid expression");
    assert_eq!(bracketed, "lower([name])");
    assert!(bound.is_empty());
}

#[test]
fn the_parameters_of_the_slots_continue_one_numbering() {
    let call = Expr::<String>::fragments("f(")
        .value("a")
        .text(", ")
        .operand(UserName.eq("b"))
        .text(", ")
        .value(7i64)
        .text(")")
        .end();

    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<BracketDialect>(&mut sql, &mut params, call.kind(), &[])
        .expect("a valid expression");

    assert_eq!(sql, "f($1, ([name] = $2), $3)");
    assert_eq!(
        params,
        vec![
            SqlVal::Text("a".to_owned()),
            SqlVal::Text("b".to_owned()),
            SqlVal::I64(7),
        ]
    );
}

#[test]
fn the_markers_of_the_operands_fold_into_the_expression() {
    // A column operand reads one value per row of its table.
    let _: Expr<String, User, NonAggregate, ()> = Expr::<String>::fragments("lower(")
        .operand(UserName)
        .text(")")
        .end();

    // An aggregate operand keeps the whole call an aggregate.
    let _: Expr<f64, Order, Aggregate, ()> = Expr::<f64>::fragments("abs(")
        .operand(sum(OrderTotal))
        .text(")")
        .end();

    // Operands of two tables claim both of them.
    let _: Expr<String, (User, Order), NonAggregate, ()> = Expr::<String>::fragments("concat(")
        .operand(UserName)
        .text(", ")
        .operand(OrderTotal)
        .text(")")
        .end();
}

#[test]
fn a_fragments_expression_projects_from_a_statement() {
    let lowered = Expr::<String>::fragments("lower(")
        .operand(UserName)
        .text(")")
        .end();

    let Ok(built) = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, lowered))
        .build()
    else {
        panic!("the statement is valid");
    };

    assert!(built.sql.contains("lower(\"name\")"));
}

#[test]
fn the_validation_of_a_statement_reaches_the_operands_of_a_slot() {
    // The operand names the `orders` table with its scope dropped, so the type
    // system lets it through while the statement never introduces the table.
    let foreign = Expr::<f64>::fragments("abs(")
        .operand(Expr::column::<OrderTotal>().without_scope())
        .text(")")
        .end();

    let result = SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, foreign))
        .build();

    assert!(matches!(
        result,
        Err(BuildError::UnresolvedQualifier { name: "orders" })
    ));
}
