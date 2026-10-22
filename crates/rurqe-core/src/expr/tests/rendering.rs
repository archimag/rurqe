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
fn filtered_and_distinct_aggregates_render_sql() {
    let (sql, params) = render(count_distinct(UserId).kind());
    assert_eq!(sql, "COUNT(DISTINCT \"id\")");
    assert!(params.is_empty());

    let (sql, params) = render(Expr::count::<UserId>().filter_where(UserId.gt(10i64)).kind());
    assert_eq!(sql, "COUNT(\"id\") FILTER (WHERE (\"id\" > ?))");
    assert_eq!(params, vec![SqlVal::I64(10)]);

    let (sql, params) = render(
        sum_distinct(UserId)
            .filter_where(UserId.gt(5i64))
            .filter_where(UserId.lt(50i64))
            .kind(),
    );
    assert_eq!(
        sql,
        "SUM(DISTINCT \"id\") FILTER (WHERE ((\"id\" > ?) AND (\"id\" < ?)))"
    );
    assert_eq!(params, vec![SqlVal::I64(5), SqlVal::I64(50)]);
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
            ExprKind::Column {
                table: "users",
                name: "name",
            },
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
    let fragments = parse_raw_sql("strftime('%Y-%m', date) = ?", 1)
        .expect("one placeholder binds one parameter");
    assert_eq!(fragments, vec!["strftime('%Y-%m', date) = ", ""]);

    let escaped = parse_raw_sql("data ?? 'key' AND age > ?", 1)
        .expect("escaped markers are not placeholders");
    assert_eq!(escaped, vec!["data ? 'key' AND age > ", ""]);

    let braces = parse_raw_sql("foo = {} AND bar = {}", 2).expect("two brace placeholders");
    assert_eq!(braces, vec!["foo = ", " AND bar = ", ""]);

    let braces_escaped = parse_raw_sql("data ?? 'key' AND age > {}", 1)
        .expect("escaped markers are not placeholders");
    assert_eq!(braces_escaped, vec!["data ? 'key' AND age > ", ""]);

    let zero_params =
        parse_raw_sql("data ?? 'key'", 0).expect("escaped markers need no parameter");
    assert_eq!(zero_params, vec!["data ? 'key'"]);
}

#[test]
fn raw_sql_rejects_mixed_placeholder_styles() {
    assert_eq!(
        parse_raw_sql("a = ? AND b = {}", 2),
        Err(RawSqlError::MixedPlaceholders)
    );
    assert!(RawSqlError::MixedPlaceholders.to_string().contains("mixes"));
}

#[test]
fn raw_sql_reports_placeholder_count_mismatches() {
    assert_eq!(
        parse_raw_sql("a = ?", 0),
        Err(RawSqlError::PlaceholderCount {
            expected: 1,
            provided: 0,
        })
    );
    assert_eq!(
        parse_raw_sql("a = {}", 0),
        Err(RawSqlError::PlaceholderCount {
            expected: 1,
            provided: 0,
        })
    );
    assert_eq!(
        parse_raw_sql("a = 1", 2),
        Err(RawSqlError::PlaceholderCount {
            expected: 0,
            provided: 2,
        })
    );
    assert!(parse_raw_sql("a = ?", 0)
        .expect_err("the template binds one parameter")
        .to_string()
        .contains("expects 1 parameter"));
}

#[test]
fn empty_membership_renders_a_constant_predicate() {
    let (sql, params) = render(Expr::column::<UserId>().in_list(Vec::<i64>::new()).kind());
    assert_eq!(sql, "1 = 0");
    assert!(params.is_empty());

    let (sql, params) = render(Expr::column::<UserId>().not_in_list(Vec::<i64>::new()).kind());
    assert_eq!(sql, "1 = 1");
    assert!(params.is_empty());
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

/// A dialect without `ILIKE`, which folds both operands with `LOWER` instead.
struct LowerDialect;

impl crate::dialect::Dialect for LowerDialect {
    const SUPPORTS_ILIKE: bool = false;

    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }
}

#[test]
fn ilike_folds_both_operands_on_a_dialect_without_ilike() {
    let (sql, params) =
        render_with::<LowerDialect>(Expr::column::<UserName>().ilike("a%").kind());

    assert_eq!(sql, "(LOWER(\"name\") LIKE LOWER(?))");
    assert_eq!(params, vec![SqlVal::Text("a%".to_owned())]);

    // A case-sensitive `LIKE` is rendered as written.
    let (sql, params) = render_with::<LowerDialect>(Expr::column::<UserName>().like("A%").kind());

    assert_eq!(sql, "(\"name\" LIKE ?)");
    assert_eq!(params, vec![SqlVal::Text("A%".to_owned())]);
}

#[test]
fn between_builds_and_renders_range_predicate() {
    let predicate = Expr::column::<UserId>().between(10i64, 20i64);
    assert_eq!(
        predicate.kind(),
        &ExprKind::Between {
            operand: Box::new(ExprKind::Column { table: "users", name: "id" }),
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
            operand: Box::new(ExprKind::Column { table: "users", name: "id" }),
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

/// A dialect quoting identifiers with brackets and numbering placeholders with
/// `#`, used to check that an expression tree carries no dialect of its own.
struct BracketDialect;

impl crate::dialect::Dialect for BracketDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('[');
        out.push_str(ident);
        out.push(']');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        use core::fmt::Write;
        let _ = write!(out, "#{index}");
    }
}

/// Renders `kind` with the dialect `D` into its SQL text and parameters.
fn render_with<D: crate::dialect::Dialect>(kind: &ExprKind) -> (String, Vec<SqlVal>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<D>(&mut sql, &mut params, kind, &[]);
    (sql, params)
}

#[test]
fn one_expression_tree_renders_under_every_dialect() {
    let predicate = Expr::column::<UserName>().eq("root");

    let (mock_sql, mock_params) = render(predicate.kind());
    let (bracket_sql, bracket_params) = render_with::<BracketDialect>(predicate.kind());

    assert_eq!(mock_sql, "(\"name\" = ?)");
    assert_eq!(bracket_sql, "([name] = #1)");
    assert_eq!(mock_params, vec![SqlVal::Text("root".to_owned())]);
    assert_eq!(bracket_params, mock_params);
}

#[test]
fn a_nested_statement_renders_with_the_dialect_of_its_enclosing_expression() {
    let inner = crate::query::SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
        .where_(Expr::column::<UserActive>().eq(true));

    let predicate = Expr::exists(inner);

    let (mock_sql, mock_params) = render(predicate.kind());
    let (bracket_sql, bracket_params) = render_with::<BracketDialect>(predicate.kind());

    assert_eq!(
        mock_sql,
        "(EXISTS (SELECT \"id\" FROM \"users\" WHERE (\"active\" = ?)))"
    );
    assert_eq!(
        bracket_sql,
        "(EXISTS (SELECT [id] FROM [users] WHERE ([active] = #1)))"
    );
    assert_eq!(mock_params, vec![SqlVal::Bool(true)]);
    assert_eq!(bracket_params, mock_params);
}
