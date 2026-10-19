use super::*;

#[test]
fn operators_render_their_sql_text() {
    assert_eq!(BinaryOp::Eq.as_str(), "=");
    assert_eq!(BinaryOp::Ne.as_str(), "<>");
    assert_eq!(BinaryOp::Gt.as_str(), ">");
    assert_eq!(BinaryOp::Lt.as_str(), "<");
    assert_eq!(BinaryOp::And.as_str(), "AND");
    assert_eq!(BinaryOp::Or.as_str(), "OR");
}

#[test]
fn aggregate_names_render_sql_identifiers() {
    assert_eq!(AggregateFunc::Count.as_str(), "COUNT");
    assert_eq!(AggregateFunc::Sum.as_str(), "SUM");
    assert_eq!(AggregateFunc::Avg.as_str(), "AVG");
    assert_eq!(AggregateFunc::Min.as_str(), "MIN");
    assert_eq!(AggregateFunc::Max.as_str(), "MAX");
}

#[test]
fn null_predicates_render_is_null_and_is_not_null() {
    assert_eq!(
        render(Expr::column::<UserName>().is_null().kind()).0,
        "(\"name\" IS NULL)"
    );
    assert_eq!(
        render(Expr::column::<UserName>().is_not_null().kind()).0,
        "(\"name\" IS NOT NULL)"
    );
}

#[test]
fn function_calls_render_name_and_arguments() {
    let kind = ExprKind::Function {
        name: "COALESCE",
        args: vec![
            ExprKind::Column { name: "name" },
            ExprKind::Value(SqlVal::Text("anonymous".to_owned())),
        ],
    };

    let (sql, params) = render(&kind);

    assert_eq!(sql, "COALESCE(\"name\", ?)");
    assert_eq!(params, vec![SqlVal::Text("anonymous".to_owned())]);
}

#[test]
fn case_expression_renders_when_then_else_and_end() {
    let expr = Expr::<String>::case()
        .when(Expr::column::<UserId>().gt(100i64), "vip")
        .else_("regular")
        .end();

    let (sql, params) = render(expr.kind());

    assert_eq!(sql, "CASE WHEN (\"id\" > ?) THEN ? ELSE ? END");
    assert_eq!(
        params,
        vec![
            SqlVal::I64(100),
            SqlVal::Text("vip".to_owned()),
            SqlVal::Text("regular".to_owned()),
        ]
    );
}

#[test]
fn case_expression_without_else_renders_bare_branches() {
    let expr = Expr::<String>::case()
        .when(Expr::column::<UserActive>().eq(true), "active")
        .end();

    let (sql, _) = render(expr.kind());

    assert_eq!(sql, "CASE WHEN (\"active\" = ?) THEN ? END");
}

#[test]
fn exists_predicate_renders_nested_query_with_continued_placeholders() {
    let sub = crate::query::SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserActive>().eq(true));

    let (sql, params) = render(Expr::exists(sub).kind());

    assert_eq!(
        sql,
        "(EXISTS (SELECT \"id\" FROM \"users\" WHERE (\"active\" = ?)))"
    );
    assert_eq!(params, vec![SqlVal::Bool(true)]);
}

#[test]
fn in_subquery_predicates_render_membership_and_negation() {
    let sub = || {
        crate::query::SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
            .where_(Expr::column::<UserId>().gt(1i64))
    };

    let (sql, params) = render(Expr::column::<UserId>().in_subquery(sub()).kind());
    assert_eq!(
        sql,
        "(\"id\" IN (SELECT \"id\" FROM \"users\" WHERE (\"id\" > ?)))"
    );
    assert_eq!(params, vec![SqlVal::I64(1)]);

    let (sql, _) = render(Expr::column::<UserId>().not_in_subquery(sub()).kind());
    assert_eq!(
        sql,
        "(\"id\" NOT IN (SELECT \"id\" FROM \"users\" WHERE (\"id\" > ?)))"
    );
}

#[test]
fn not_exists_predicate_renders_not_keyword() {
    let sub = crate::query::SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,));

    let (sql, params) = render(Expr::not_exists(sub).kind());

    assert_eq!(sql, "(NOT EXISTS (SELECT \"id\" FROM \"users\"))");
    assert!(params.is_empty());
}

#[test]
fn raw_sql_splits_fragments_and_preserves_escaped_question_marks() {
    let fragments = parse_raw_sql("strftime('%Y-%m', date) = ?", 1);
    assert_eq!(fragments, vec!["strftime('%Y-%m', date) = ", ""]);

    let escaped = parse_raw_sql("data ?? 'key' AND age > ?", 1);
    assert_eq!(escaped, vec!["data ? 'key' AND age > ", ""]);

    let braces = parse_raw_sql("foo = {} AND bar = {}", 2);
    assert_eq!(braces, vec!["foo = ", " AND bar = ", ""]);

    let braces_escaped = parse_raw_sql("data ?? 'key' AND age > {}", 1);
    assert_eq!(braces_escaped, vec!["data ? 'key' AND age > ", ""]);

    let zero_params = parse_raw_sql("data ?? 'key'", 0);
    assert_eq!(zero_params, vec!["data ? 'key'"]);
}

#[test]
#[should_panic(expected = "mixes")]
fn raw_sql_rejects_mixed_placeholder_styles() {
    parse_raw_sql("a = ? AND b = {}", 2);
}

#[test]
#[should_panic(expected = "expects 1 parameter")]
fn raw_sql_rejects_question_mark_with_zero_parameters() {
    parse_raw_sql("a = ?", 0);
}

#[test]
#[should_panic(expected = "expects 1 parameter")]
fn raw_sql_rejects_brace_placeholder_with_zero_parameters() {
    parse_raw_sql("a = {}", 0);
}

#[test]
fn like_and_ilike_render_patterns_with_bound_parameters() {
    let (sql, params) = render(Expr::column::<UserName>().like("A%").kind());
    assert_eq!(sql, "(\"name\" LIKE ?)");
    assert_eq!(params, vec![SqlVal::Text("A%".to_owned())]);

    let (sql, params) = render(Expr::column::<UserName>().ilike("a%").kind());
    assert_eq!(sql, "(\"name\" ILIKE ?)");
    assert_eq!(params, vec![SqlVal::Text("a%".to_owned())]);
}

#[test]
fn between_builds_and_renders_range_predicate() {
    let predicate = Expr::column::<UserId>().between(10i64, 20i64);
    assert_eq!(
        predicate.kind(),
        &ExprKind::Between {
            operand: Box::new(ExprKind::Column { name: "id" }),
            low: Box::new(ExprKind::Value(SqlVal::I64(10))),
            high: Box::new(ExprKind::Value(SqlVal::I64(20))),
        }
    );

    let (sql, params) = render(predicate.kind());
    assert_eq!(sql, "(\"id\" BETWEEN ? AND ?)");
    assert_eq!(params, vec![SqlVal::I64(10), SqlVal::I64(20)]);
}

#[test]
fn in_list_builds_and_renders_membership() {
    let predicate = Expr::column::<UserId>().in_list(vec![1i64, 2i64, 3i64]);
    assert_eq!(
        predicate.kind(),
        &ExprKind::InList {
            operand: Box::new(ExprKind::Column { name: "id" }),
            values: vec![
                ExprKind::Value(SqlVal::I64(1)),
                ExprKind::Value(SqlVal::I64(2)),
                ExprKind::Value(SqlVal::I64(3)),
            ],
            negated: false,
        }
    );

    let (sql, params) = render(predicate.kind());
    assert_eq!(sql, "(\"id\" IN (?, ?, ?))");
    assert_eq!(
        params,
        vec![SqlVal::I64(1), SqlVal::I64(2), SqlVal::I64(3)]
    );
}

#[test]
fn not_in_list_renders_negated_membership() {
    let (sql, params) = render(Expr::column::<UserName>().not_in_list(vec!["a", "b"]).kind());
    assert_eq!(sql, "(\"name\" NOT IN (?, ?))");
    assert_eq!(
        params,
        vec![SqlVal::Text("a".to_owned()), SqlVal::Text("b".to_owned())]
    );
}

#[test]
fn predicates_compose_with_boolean_operators() {
    let predicate = Expr::column::<UserName>()
        .like("A%")
        .and(Expr::column::<UserId>().between(1i64, 9i64))
        .or(Expr::column::<UserName>().not_in_list(vec!["x"]));

    let (sql, params) = render(predicate.kind());

    assert_eq!(
        sql,
        "(((\"name\" LIKE ?) AND (\"id\" BETWEEN ? AND ?)) OR (\"name\" NOT IN (?)))"
    );
    assert_eq!(
        params,
        vec![
            SqlVal::Text("A%".to_owned()),
            SqlVal::I64(1),
            SqlVal::I64(9),
            SqlVal::Text("x".to_owned()),
        ]
    );
}

#[test]
fn coalesce_binds_the_fallback_parameter() {
    let expr = coalesce(UserName, "anonymous");

    let (sql, params) = render(expr.kind());

    assert_eq!(sql, "COALESCE(\"name\", ?)");
    assert_eq!(params, vec![SqlVal::Text("anonymous".to_owned())]);
}
