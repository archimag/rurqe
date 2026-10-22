//! PostgreSQL-specific expression extensions and text search functions.

use rurqe_core::expr::{render_expr, Expr, SqlVal};
use rurqe_core::schema::Column;

use crate::dialect::Postgres;

/// PostgreSQL-specific expressions and operators.
pub trait PgExpressions: Column {
    /// POSIX regex match (`~`).
    fn regex_match(self, pattern: &str) -> Expr<bool, Self::Table>;

    /// POSIX regex case-insensitive match (`~*`).
    fn regex_match_ci(self, pattern: &str) -> Expr<bool, Self::Table>;

    /// POSIX regex does not match (`!~`).
    fn regex_not_match(self, pattern: &str) -> Expr<bool, Self::Table>;

    /// POSIX regex case-insensitive does not match (`!~*`).
    fn regex_not_match_ci(self, pattern: &str) -> Expr<bool, Self::Table>;

    /// JSONB contains (`@>`).
    fn json_contains(self, other: &str) -> Expr<bool, Self::Table>;

    /// JSONB exists / has key (`?`).
    fn json_has_key(self, key: &str) -> Expr<bool, Self::Table>;

    /// JSONB has any keys (`?|`).
    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool, Self::Table>;

    /// Full-text search match (`@@ to_tsquery(?)`).
    fn matches_tsquery(self, query: &str) -> Expr<bool, Self::Table>;
}

impl<C: Column> PgExpressions for C {
    fn regex_match(self, pattern: &str) -> Expr<bool, Self::Table> {
        column_binary_op::<C>("~", pattern)
    }

    fn regex_match_ci(self, pattern: &str) -> Expr<bool, Self::Table> {
        column_binary_op::<C>("~*", pattern)
    }

    fn regex_not_match(self, pattern: &str) -> Expr<bool, Self::Table> {
        column_binary_op::<C>("!~", pattern)
    }

    fn regex_not_match_ci(self, pattern: &str) -> Expr<bool, Self::Table> {
        column_binary_op::<C>("!~*", pattern)
    }

    fn json_contains(self, other: &str) -> Expr<bool, Self::Table> {
        column_binary_op::<C>("@>", other)
    }

    fn json_has_key(self, key: &str) -> Expr<bool, Self::Table> {
        column_binary_op::<C>("?", key)
    }

    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool, Self::Table> {
        let elements: Vec<String> = keys.iter().map(|key| quote_array_element(key)).collect();
        let array = format!("{{{}}}", elements.join(","));
        column_binary_op::<C>("?|", &array)
    }

    fn matches_tsquery(self, query: &str) -> Expr<bool, Self::Table> {
        Expr::raw_fragments_with_scope(
            vec![
                format!("({} @@ to_tsquery(", render_column::<C>()),
                "))".to_owned(),
            ],
            vec![SqlVal::Text(query.to_owned())],
        )
    }
}

/// Renders the SQL reference of a column marker through the PostgreSQL dialect.
///
/// Rendering goes through [`render_expr`], so the identifier is quoted and its
/// embedded double quotes are doubled, while an alias-qualified column keeps its
/// alias prefix (`"u"."id"`).
fn render_column<C: Column>() -> String {
    // No relation is in scope, so a bare column stays unqualified while an
    // alias-qualified one keeps its `"alias"."column"` prefix.
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<Postgres>(&mut sql, &mut params, &C::expr_kind(), &[]);
    sql
}

/// Builds a `(<column> <op> ?)` predicate over the column `C`.
fn column_binary_op<C: Column>(op: &str, val: &str) -> Expr<bool, C::Table> {
    Expr::raw_fragments_with_scope(
        vec![format!("({} {op} ", render_column::<C>()), ")".to_owned()],
        vec![SqlVal::Text(val.to_owned())],
    )
}

/// Quotes a single element of a PostgreSQL array literal.
///
/// Elements are double-quoted so that separators stay inert, and both `"` and
/// `\` are backslash-escaped as the array literal syntax requires.
fn quote_array_element(element: &str) -> String {
    let mut quoted = String::with_capacity(element.len() + 2);
    quoted.push('"');
    for ch in element.chars() {
        if ch == '"' || ch == '\\' {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// Builds a `to_tsvector(?)` expression.
pub fn to_tsvector(text: &str) -> Expr<String> {
    Expr::raw_fragments(
        vec!["to_tsvector(".to_owned(), ")".to_owned()],
        vec![SqlVal::Text(text.to_owned())],
    )
}

/// Builds a `to_tsquery(?)` expression.
pub fn to_tsquery(query: &str) -> Expr<String> {
    Expr::raw_fragments(
        vec!["to_tsquery(".to_owned(), ")".to_owned()],
        vec![SqlVal::Text(query.to_owned())],
    )
}

/// Builds a `plainto_tsquery(?)` expression.
pub fn plainto_tsquery(query: &str) -> Expr<String> {
    Expr::raw_fragments(
        vec!["plainto_tsquery(".to_owned(), ")".to_owned()],
        vec![SqlVal::Text(query.to_owned())],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_core::expr::ExprKind;
    use rurqe_core::schema::AliasedCol;
    use rurqe_core::table;

    table! {
        Doc => "docs" {
            DocContent => "content": String,
            DocMetadata => "metadata": String,
            DocQuoted => "a\"b": String,
        }
    }

    rurqe_core::table_alias! {
        /// The `d` alias.
        DocAlias => "d"
    }

    #[test]
    fn pg_expressions_build_expected_raw_ast() {
        let regex = DocContent.regex_match("^start");
        assert!(matches!(regex.kind(), ExprKind::Raw { .. }));

        let json = DocMetadata.json_contains(r#"{"active": true}"#);
        assert!(matches!(json.kind(), ExprKind::Raw { .. }));

        let ts = DocContent.matches_tsquery("english & rust");
        assert!(matches!(ts.kind(), ExprKind::Raw { .. }));
    }

    #[test]
    fn pg_expressions_carry_column_table_scope() {
        let regex: Expr<bool, Doc> = DocContent.regex_match("^start");
        assert!(matches!(regex.kind(), ExprKind::Raw { .. }));

        let regex_ci: Expr<bool, Doc> = DocContent.regex_match_ci("^start");
        assert!(matches!(regex_ci.kind(), ExprKind::Raw { .. }));

        let not_match: Expr<bool, Doc> = DocContent.regex_not_match("^start");
        assert!(matches!(not_match.kind(), ExprKind::Raw { .. }));

        let not_match_ci: Expr<bool, Doc> = DocContent.regex_not_match_ci("^start");
        assert!(matches!(not_match_ci.kind(), ExprKind::Raw { .. }));

        let json: Expr<bool, Doc> = DocMetadata.json_contains(r#"{"active": true}"#);
        assert!(matches!(json.kind(), ExprKind::Raw { .. }));

        let json_key: Expr<bool, Doc> = DocMetadata.json_has_key("active");
        assert!(matches!(json_key.kind(), ExprKind::Raw { .. }));

        let json_keys: Expr<bool, Doc> = DocMetadata.json_has_any_keys(&["a", "b"]);
        assert!(matches!(json_keys.kind(), ExprKind::Raw { .. }));

        let ts: Expr<bool, Doc> = DocContent.matches_tsquery("english & rust");
        assert!(matches!(ts.kind(), ExprKind::Raw { .. }));
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
    fn column_references_are_quoted_and_escaped() {
        assert_eq!(render_column::<DocContent>(), "\"content\"");
        assert_eq!(render_column::<DocQuoted>(), "\"a\"\"b\"");
    }

    #[test]
    fn aliased_column_references_keep_their_alias_prefix() {
        assert_eq!(
            render_column::<AliasedCol<DocAlias, DocContent>>(),
            "\"d\".\"content\""
        );
    }

    #[test]
    fn array_elements_are_quoted_and_escaped() {
        assert_eq!(quote_array_element("plain"), "\"plain\"");
        assert_eq!(quote_array_element("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_array_element("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn json_has_any_keys_binds_a_quoted_array_literal() {
        let expr = DocMetadata.json_has_any_keys(&["a", "b\"c"]);
        match expr.kind() {
            ExprKind::Raw { fragments, params } => {
                assert_eq!(fragments.first().map(String::as_str), Some("(\"metadata\" ?| "));
                assert_eq!(
                    params,
                    &vec![SqlVal::Text("{\"a\",\"b\\\"c\"}".to_owned())]
                );
            }
            other => panic!("expected a raw expression, got {other:?}"),
        }
    }
}
