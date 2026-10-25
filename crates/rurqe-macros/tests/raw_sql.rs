use rurqe_core::expr::{Expr, ExprKind, Json, SqlVal};
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

#[test]
fn sql_macro_binds_optional_parameters_through_to_sql_val() {
    let present = sql!(bool, "age = ?", Some(18i64));
    assert_eq!(
        present.kind(),
        &ExprKind::Raw {
            fragments: vec!["age = ".into(), "".into()],
            params: vec![SqlVal::I64(18)],
        }
    );

    let absent = sql!(bool, "age = ?", None::<i64>);
    assert_eq!(
        absent.kind(),
        &ExprKind::Raw {
            fragments: vec!["age = ".into(), "".into()],
            params: vec![SqlVal::Null],
        }
    );

    let owned = sql!(bool, "name = ?", Some(String::from("root")));
    assert_eq!(
        owned.kind(),
        &ExprKind::Raw {
            fragments: vec!["name = ".into(), "".into()],
            params: vec![SqlVal::Text("root".into())],
        }
    );
}

#[test]
fn sql_macro_binds_json_parameters_through_to_sql_val() {
    let owned = sql!(
        bool,
        "payload @> ?",
        Json(serde_json::json!({ "role": "admin" }))
    );
    assert_eq!(
        owned.kind(),
        &ExprKind::Raw {
            fragments: vec!["payload @> ".into(), "".into()],
            params: vec![SqlVal::Json(serde_json::json!({ "role": "admin" }))],
        }
    );

    let payload = Json(serde_json::json!([1, 2, 3]));
    let borrowed = sql!(bool, "payload @> ?", &payload);
    assert_eq!(
        borrowed.kind(),
        &ExprKind::Raw {
            fragments: vec!["payload @> ".into(), "".into()],
            params: vec![SqlVal::Json(serde_json::json!([1, 2, 3]))],
        }
    );
    assert_eq!(*payload, serde_json::json!([1, 2, 3]));
}

#[test]
fn sql_macro_binds_borrowed_values_without_moving_them() {
    let name = String::from("root");
    let expr = sql!(bool, "name = ? AND parent = ?", &name, name.as_str());
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["name = ".into(), " AND parent = ".into(), "".into()],
            params: vec![
                SqlVal::Text("root".into()),
                SqlVal::Text("root".into()),
            ],
        }
    );
    assert_eq!(name, "root");
}

#[test]
fn sql_macro_unescapes_braces_in_json_literals() {
    let expr: Expr<bool> = sql!("payload @> '{{\"role\": \"admin\"}}'::jsonb");
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec!["payload @> '{\"role\": \"admin\"}'::jsonb".into()],
            params: vec![],
        }
    );
}

#[test]
fn sql_macro_unescapes_braces_around_parameters() {
    let expr = sql!(bool, "payload @> '{{\"role\": ?}}'::jsonb", "admin");
    assert_eq!(
        expr.kind(),
        &ExprKind::Raw {
            fragments: vec![
                "payload @> '{\"role\": ".into(),
                "}'::jsonb".into(),
            ],
            params: vec![SqlVal::Text("admin".into())],
        }
    );
}
