use rurqe_core::expr::{Expr, ExprKind, SqlVal};
use rurqe_macros::{raw_sql, sql};

#[test]
fn sql_macro_constructs_parameterless_expression() {
    let expr: Expr<bool> = sql!("1 = 1");
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["1 = 1".into()],
            params: vec![],
        }
    );
}

#[test]
fn sql_macro_accepts_explicit_type_and_question_marks() {
    let expr = sql!(bool, "strftime('%Y-%m', created_at) = ?", "2024-01");
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["strftime('%Y-%m', created_at) = ".into(), "".into()],
            params: vec![SqlVal::Text("2024-01".into())],
        }
    );
}

#[test]
fn sql_macro_accepts_angle_bracket_type() {
    let expr = sql!(<i64> "COUNT(*)");
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["COUNT(*)".into()],
            params: vec![],
        }
    );
}

#[test]
fn sql_macro_handles_escaped_question_marks_and_multiple_arguments() {
    let expr = sql!(bool, "data ?? 'role' AND age >= ?", 18i64);
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["data ? 'role' AND age >= ".into(), "".into()],
            params: vec![SqlVal::I64(18)],
        }
    );
}

#[test]
fn raw_sql_macro_alias_functions_identically() {
    let expr: Expr<bool> = raw_sql!("foo = {} AND bar = {}", 1i64, true);
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["foo = ".into(), " AND bar = ".into(), "".into()],
            params: vec![SqlVal::I64(1), SqlVal::Bool(true)],
        }
    );
}
