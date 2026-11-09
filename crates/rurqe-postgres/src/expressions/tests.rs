//! The tests of the PostgreSQL expression extensions.

use super::{
    plainto_tsquery, to_tsquery, to_tsvector, JsonKey, PgArrayExpressions, PgJson,
    PgJsonExpressions, PgJsonValue, PgJsonb, PgJsonbExpressions, PgJsonbValue, PgRegexExpressions,
    PgTextSearchExpressions, PgTextSearchValue, PgTextValue,
};
use crate::dialect::Postgres;
use rurqe_core::expr::{render_expr, Expr, ExprKind, NonAggregate, SqlVal, ToSqlVal};
use rurqe_core::query::MakeNullable;
use rurqe_core::row::{FromSqlValue, RowDecodeError};
use rurqe_core::schema::{AliasedCol, Column, TableRef};
use rurqe_core::table;

#[cfg(feature = "chrono")]
use super::{PgDateTruncUnit, PgTemporalExpressions};
#[cfg(feature = "chrono")]
use rurqe_core::query::SelectQuery;

table!(
    Doc => "docs" {
        DocContent => "content": String,
        DocMetadata => "metadata": PgJsonb,
        DocLegacy => "legacy": PgJson,
        DocQuoted => "a\"b": String,
    }
);

table!(
    Post => "posts" {
        PostTags => "tags": Vec<String>,
        PostScores => "scores": Option<Vec<i64>>,
    }
);

rurqe_core::table_alias! {
    /// The `d` alias.
    DocAlias => "d"
}

/// Renders `expr` under the PostgreSQL dialect against `relations`.
fn render(expr: &ExprKind, relations: &[TableRef]) -> (String, Vec<SqlVal>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<Postgres>(&mut sql, &mut params, expr, relations).expect("valid expression");
    (sql, params)
}

#[test]
fn pg_expressions_build_dialect_operators() {
    assert!(matches!(
        DocContent.regex_match("^start").kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocContent.regex_match_ci("^start").kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocContent.regex_not_match("^start").kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocContent.regex_not_match_ci("^start").kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocMetadata.json_contains(r#"{"active": true}"#).kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocMetadata.json_has_key("active").kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocMetadata.json_has_any_keys(&["a", "b"]).kind(),
        ExprKind::DialectOp { .. }
    ));
    assert!(matches!(
        DocContent.matches_tsquery("english & rust").kind(),
        ExprKind::DialectOp { .. }
    ));
}

#[test]
fn pg_expressions_carry_column_table_scope() {
    let regex: Expr<bool, Doc> = DocContent.regex_match("^start");
    assert!(matches!(regex.kind(), ExprKind::DialectOp { .. }));

    let json: Expr<bool, Doc> = DocMetadata.json_contains(r#"{"active": true}"#);
    assert!(matches!(json.kind(), ExprKind::DialectOp { .. }));

    let ts: Expr<bool, Doc> = DocContent.matches_tsquery("english & rust");
    assert!(matches!(ts.kind(), ExprKind::DialectOp { .. }));
}

#[test]
fn text_search_functions_build_raw_ast() {
    let v = to_tsvector("fat cats");
    assert!(matches!(v.kind(), ExprKind::Raw { .. }));

    let q = to_tsquery("cat & rat");
    assert!(matches!(q.kind(), ExprKind::Raw { .. }));

    let p = plainto_tsquery("the fat cat");
    assert!(matches!(p.kind(), ExprKind::Raw { .. }));
}

#[test]
fn column_references_are_qualified_against_the_statement() {
    let regex = DocContent.regex_match("^start");

    let (bare, params) = render(regex.kind(), &[TableRef::plain("docs")]);
    assert_eq!(bare, "(\"content\" ~ $1)");
    assert_eq!(params, vec![SqlVal::Text("^start".to_owned())]);

    let (qualified, _) = render(
        regex.kind(),
        &[TableRef::plain("docs"), TableRef::plain("other")],
    );
    assert_eq!(qualified, "(\"docs\".\"content\" ~ $1)");
}

#[test]
fn column_references_are_quoted_and_escaped() {
    let quoted = DocQuoted.regex_match("^start");
    let (sql, _) = render(quoted.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(\"a\"\"b\" ~ $1)");
}

#[test]
fn aliased_column_references_keep_their_alias_prefix() {
    let expr = AliasedCol::<DocAlias, DocContent>::new().regex_match("^start");
    let (sql, _) = render(expr.kind(), &[TableRef::aliased("docs", "d")]);
    assert_eq!(sql, "(\"d\".\"content\" ~ $1)");
}

#[test]
fn matches_tsquery_binds_the_query_as_a_nested_function_call() {
    let expr = DocContent.matches_tsquery("english & rust");
    let (sql, params) = render(expr.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(to_tsvector(\"content\") @@ to_tsquery($1))");
    assert_eq!(params, vec![SqlVal::Text("english & rust".to_owned())]);
}

#[test]
fn matches_tsquery_qualifies_the_wrapped_column_reference() {
    let expr = DocContent.matches_tsquery("english & rust");

    // The document is built from the column reference, so a statement
    // addressing several relations qualifies it inside the call exactly
    // like a bare reference of the same column.
    let (qualified, params) = render(
        expr.kind(),
        &[TableRef::plain("docs"), TableRef::plain("other")],
    );
    assert_eq!(
        qualified,
        "(to_tsvector(\"docs\".\"content\") @@ to_tsquery($1))"
    );
    assert_eq!(params, vec![SqlVal::Text("english & rust".to_owned())]);

    // An aliased column keeps its alias prefix inside the call.
    let aliased = AliasedCol::<DocAlias, DocContent>::new().matches_tsquery("rust");
    let (sql, _) = render(aliased.kind(), &[TableRef::aliased("docs", "d")]);
    assert_eq!(sql, "(to_tsvector(\"d\".\"content\") @@ to_tsquery($1))");
}

#[test]
fn json_has_any_keys_binds_one_array_parameter() {
    let expr = DocMetadata.json_has_any_keys(&["a", "b\"c"]);
    let (sql, params) = render(expr.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(\"metadata\" ?| $1)");
    assert_eq!(
        params,
        vec![SqlVal::Array(vec![
            SqlVal::Text("a".to_owned()),
            SqlVal::Text("b\"c".to_owned()),
        ])]
    );
}

#[test]
fn array_operators_build_dialect_operators() {
    assert!(matches!(
        PostTags.array_contains(vec!["rust".to_owned()]).kind(),
        ExprKind::DialectOp { op: "@>", .. }
    ));
    assert!(matches!(
        PostTags.array_contained_by(vec!["rust".to_owned()]).kind(),
        ExprKind::DialectOp { op: "<@", .. }
    ));
    assert!(matches!(
        PostScores.array_overlaps(vec![1i64]).kind(),
        ExprKind::DialectOp { op: "&&", .. }
    ));
}

#[test]
fn array_operators_carry_the_column_scope() {
    let contains: Expr<bool, Post> = PostTags.array_contains(vec!["rust".to_owned()]);
    let any: Expr<bool, Post> = PostTags.any("rust".to_owned());
    let all: Expr<bool, Post> = PostScores.all(0i64);
    let _ = (contains, any, all);
}

#[test]
fn array_operators_render_against_the_statement() {
    let expr = PostTags.array_contains(vec!["rust".to_owned()]);

    let (bare, params) = render(expr.kind(), &[TableRef::plain("posts")]);
    assert_eq!(bare, "(\"tags\" @> $1)");
    assert_eq!(
        params,
        vec![SqlVal::Array(vec![SqlVal::Text("rust".to_owned())])]
    );

    // A statement addressing several relations qualifies the reference.
    let (qualified, _) = render(
        expr.kind(),
        &[TableRef::plain("posts"), TableRef::plain("other")],
    );
    assert_eq!(qualified, "(\"posts\".\"tags\" @> $1)");
}

#[test]
fn array_quantifiers_render_any_and_all() {
    let any = PostTags.any("rust".to_owned());
    let (sql, params) = render(any.kind(), &[TableRef::plain("posts")]);
    assert_eq!(sql, "($1 = ANY(\"tags\"))");
    assert_eq!(params, vec![SqlVal::Text("rust".to_owned())]);

    let all = PostScores.all(0i64);
    let (sql, params) = render(all.kind(), &[TableRef::plain("posts")]);
    assert_eq!(sql, "($1 = ALL(\"scores\"))");
    assert_eq!(params, vec![SqlVal::I64(0)]);
}

#[test]
fn array_operators_keep_the_dialect_marker_of_their_operand() {
    // The operand carries the dialect it was built for, so the marker
    // survives the operator and reaches the predicate the statement checks.
    let tag = Expr::<String, (), NonAggregate, Postgres>::from_kind(ExprKind::Value(SqlVal::Text(
        "rust".to_owned(),
    )));
    let score = Expr::<i64, (), NonAggregate, Postgres>::from_kind(ExprKind::Value(SqlVal::I64(0)));

    let any: Expr<bool, Post, NonAggregate, Postgres> = PostTags.any(tag);
    let all: Expr<bool, Post, NonAggregate, Postgres> = PostScores.all(score);
    let _ = (any, all);
}

#[test]
fn json_extraction_reads_an_object_key() {
    let value = DocMetadata.json_get("owner");
    let (sql, params) = render(value.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(\"metadata\" -> $1)");
    assert_eq!(params, vec![SqlVal::Text("owner".to_owned())]);

    let text = DocMetadata.json_get_text("owner");
    let (sql, params) = render(text.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(\"metadata\" ->> $1)");
    assert_eq!(params, vec![SqlVal::Text("owner".to_owned())]);
}

#[test]
fn json_extraction_reads_an_array_index() {
    // The index reaches the server as an integer literal, so the operator
    // resolves to its `integer` overload instead of typing a bound parameter
    // as `text` and rejecting the encoded `i32` at bind time.
    let value = DocMetadata.json_get(0i32);
    let (sql, params) = render(value.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(\"metadata\" -> 0)");
    assert!(params.is_empty());

    let text = DocMetadata.json_get_text(JsonKey::Index(1));
    let (sql, params) = render(text.kind(), &[TableRef::plain("docs")]);
    assert_eq!(sql, "(\"metadata\" ->> 1)");
    assert!(params.is_empty());
}

#[test]
fn json_extraction_keeps_a_negative_index_a_signed_integer_literal() {
    let text = DocMetadata.json_get_text(-1i32);
    let (sql, params) = render(text.kind(), &[TableRef::plain("docs")]);

    assert_eq!(sql, "(\"metadata\" ->> -1)");
    assert!(params.is_empty());
}

#[test]
fn json_extraction_carries_the_column_scope_and_qualifies_it() {
    let value: Expr<PgJsonb, Doc> = DocMetadata.json_get("owner");
    let (sql, _) = render(
        value.kind(),
        &[TableRef::plain("docs"), TableRef::plain("posts")],
    );

    assert_eq!(sql, "(\"docs\".\"metadata\" -> $1)");

    let legacy: Expr<PgJson, Doc> = DocLegacy.json_get("owner");
    let (sql, _) = render(
        legacy.kind(),
        &[TableRef::plain("docs"), TableRef::plain("posts")],
    );

    assert_eq!(sql, "(\"docs\".\"legacy\" -> $1)");
}

#[test]
fn json_extraction_yields_the_document_of_its_column() {
    // `->` returns a value of the column's own storage kind, so the extracted
    // document is `json` for a `json` column and `jsonb` for a `jsonb` one.
    // `->>` is the operator that yields text, and it yields it for either kind.
    let jsonb: Expr<PgJsonb, Doc> = DocMetadata.json_get("owner");
    let json: Expr<PgJson, Doc> = DocLegacy.json_get("owner");
    let text: Expr<String, Doc> = DocLegacy.json_get_text("owner");

    let (rendered_jsonb, _) = render(jsonb.kind(), &[TableRef::plain("docs")]);
    let (rendered_json, _) = render(json.kind(), &[TableRef::plain("docs")]);
    let (rendered_text, _) = render(text.kind(), &[TableRef::plain("docs")]);

    assert_eq!(rendered_jsonb, "(\"metadata\" -> $1)");
    assert_eq!(rendered_json, "(\"legacy\" -> $1)");
    assert_eq!(rendered_text, "(\"legacy\" ->> $1)");
}

#[test]
fn a_document_decodes_from_the_value_of_its_column() {
    assert_eq!(
        PgJson::from_sql_value(&SqlVal::Text("{\"a\": 1}".to_owned())),
        Ok(PgJson("{\"a\": 1}".to_owned()))
    );
    assert_eq!(
        PgJsonb::from_sql_value(&SqlVal::Text("[1, 2]".to_owned())),
        Ok(PgJsonb("[1, 2]".to_owned()))
    );
    assert!(matches!(
        PgJsonb::<String>::from_sql_value(&SqlVal::I64(1)),
        Err(RowDecodeError::TypeMismatch { .. })
    ));
}

#[test]
fn text_columns_satisfy_the_text_markers() {
    fn assert_text_value<T: PgTextValue>() {}
    fn assert_search_value<T: PgTextSearchValue>() {}
    fn assert_text_column<C: Column>()
    where
        C::RustType: PgTextValue + PgTextSearchValue,
    {
    }

    assert_text_value::<String>();
    assert_text_value::<Option<String>>();
    assert_text_value::<&'static str>();
    assert_text_value::<Option<&'static str>>();
    assert_search_value::<String>();
    assert_search_value::<Option<String>>();

    assert_text_column::<DocContent>();
}

#[test]
fn a_json_column_carries_the_json_marker() {
    fn assert_json_value<T: PgJsonValue>() {}
    fn assert_jsonb_value<T: PgJsonbValue>() {}
    fn assert_nullable<T: MakeNullable<Nullable = Option<T>>>() {}

    assert_json_value::<PgJson>();
    assert_json_value::<PgJson<String>>();
    assert_json_value::<Option<PgJson>>();
    assert_json_value::<PgJsonb>();
    assert_json_value::<PgJsonb<String>>();
    assert_json_value::<Option<PgJsonb>>();

    // Only a value stating the `jsonb` storage reaches the operators defined
    // for that kind alone.
    assert_jsonb_value::<PgJsonb>();
    assert_jsonb_value::<PgJsonb<String>>();
    assert_jsonb_value::<Option<PgJsonb>>();

    #[cfg(feature = "serde_json")]
    {
        assert_json_value::<serde_json::Value>();
        assert_json_value::<Option<serde_json::Value>>();
        assert_json_value::<rurqe_core::expr::Json<String>>();
        assert_json_value::<Option<rurqe_core::expr::Json<String>>>();
    }

    assert_nullable::<PgJson>();
    assert_nullable::<PgJsonb>();
    assert_eq!(
        PgJson("{\"a\": 1}").to_sql_val(),
        SqlVal::Text("{\"a\": 1}".to_owned())
    );
    assert_eq!((&PgJson("{}")).to_sql_val(), SqlVal::Text("{}".to_owned()));
    assert_eq!(
        PgJsonb("[1, 2]").to_sql_val(),
        SqlVal::Text("[1, 2]".to_owned())
    );
    assert_eq!((&PgJsonb("{}")).to_sql_val(), SqlVal::Text("{}".to_owned()));
}

#[test]
fn a_json_column_is_never_read_as_text() {
    fn assert_json_column<C: Column>()
    where
        C::RustType: PgJsonValue,
    {
    }

    assert_json_column::<DocMetadata>();
    assert_json_column::<DocLegacy>();
}
#[test]
fn array_agg_wraps_an_ordered_aggregate_in_coalesce() {
    use super::{array_agg, PgAggregateExpressions};

    let expr: Expr<Vec<String>, (), NonAggregate, Postgres> =
        array_agg(DocContent).order_by_asc(DocContent);
    let (sql, params) = render(expr.kind(), &[TableRef::plain("docs")]);

    assert_eq!(
        sql,
        "COALESCE(ARRAY_AGG(\"content\" ORDER BY \"content\" ASC), '{}')"
    );
    assert!(params.is_empty());
}

#[test]
fn string_agg_binds_the_delimiter_and_orders_inside_the_call() {
    use super::{string_agg, PgAggregateExpressions};

    let expr: Expr<String, (), NonAggregate, Postgres> =
        string_agg(DocContent, ", ").order_by_desc(DocContent);
    let (sql, params) = render(expr.kind(), &[TableRef::plain("docs")]);

    assert_eq!(
        sql,
        "COALESCE(STRING_AGG(\"docs\".\"content\", $1 ORDER BY \"content\" DESC), '')"
    );
    assert_eq!(params, vec![SqlVal::Text(", ".to_owned())]);
}

#[test]
fn an_aggregate_filter_renders_inside_the_coalesce() {
    use super::{array_agg, PgAggregateExpressions};

    let expr: Expr<Vec<String>, (), NonAggregate, Postgres> =
        array_agg(DocContent).filter_where(DocContent.regex_match("^a"));
    let (sql, params) = render(expr.kind(), &[TableRef::plain("docs")]);

    assert_eq!(
        sql,
        "COALESCE(ARRAY_AGG(\"content\") FILTER (WHERE (\"content\" ~ $1)), '{}')"
    );
    assert_eq!(params, vec![SqlVal::Text("^a".to_owned())]);
}

#[cfg(feature = "chrono")]
table!(
    /// The `workouts` table, whose timestamp column the bucket truncates.
    Workout => "workouts" {
        WorkoutStartedAt => "started_at": chrono::DateTime<chrono::Utc>,
    }
);

#[cfg(feature = "chrono")]
#[test]
fn date_trunc_renders_every_unit_as_a_literal() {
    for (unit, literal) in [
        (PgDateTruncUnit::Year, "year"),
        (PgDateTruncUnit::Month, "month"),
        (PgDateTruncUnit::Week, "week"),
        (PgDateTruncUnit::Day, "day"),
    ] {
        let expr = WorkoutStartedAt.date_trunc(unit);
        let (sql, params) = render(expr.kind(), &[TableRef::plain("workouts")]);

        assert_eq!(sql, format!("date_trunc('{literal}', \"started_at\")"));
        assert!(params.is_empty());
    }
}

#[cfg(feature = "chrono")]
#[test]
fn date_trunc_qualifies_the_column_reference() {
    let expr = WorkoutStartedAt.date_trunc(PgDateTruncUnit::Day);
    let (sql, _) = render(
        expr.kind(),
        &[TableRef::plain("workouts"), TableRef::plain("other")],
    );

    assert_eq!(sql, "date_trunc('day', \"workouts\".\"started_at\")");
}

#[cfg(feature = "chrono")]
#[test]
fn the_projection_and_the_grouping_render_the_same_bucket() {
    let bucket = WorkoutStartedAt.date_trunc(PgDateTruncUnit::Day);
    let built = SelectQuery::<Postgres>::new()
        .from::<Workout>()
        .group_by(bucket.clone())
        .select((bucket, Expr::<i64>::count_all()))
        .build()
        .expect("valid grouped query");

    assert_eq!(
        built.sql,
        "SELECT date_trunc('day', \"started_at\"), COUNT(*) FROM \"workouts\" \
         GROUP BY date_trunc('day', \"started_at\")"
    );
}

#[cfg(feature = "chrono")]
#[test]
fn the_bucket_carries_no_parameter_of_its_own() {
    let bucket = WorkoutStartedAt.date_trunc(PgDateTruncUnit::Day);
    let built = SelectQuery::<Postgres>::new()
        .from::<Workout>()
        .group_by(bucket.clone())
        .select((bucket, Expr::<i64>::count_all()))
        .having(Expr::<i64>::count_all().gt(2i64))
        .build()
        .expect("valid grouped query");

    assert_eq!(
        built.sql,
        "SELECT date_trunc('day', \"started_at\"), COUNT(*) FROM \"workouts\" \
         GROUP BY date_trunc('day', \"started_at\") HAVING (COUNT(*) > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(2)]);
}
