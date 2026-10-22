//! PostgreSQL-specific expression extensions and text search functions.

use rurqe_core::expr::{Expr, ExprKind, SqlVal};
use rurqe_core::schema::Column;

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
        Expr::from_kind(ExprKind::DialectOp {
            op: "@@",
            lhs: Box::new(C::expr_kind()),
            rhs: Box::new(to_tsquery(query).into_kind()),
        })
    }
}

/// Builds a `(<column> <op> <value>)` predicate over the column `C`.
///
/// Both the operator and the operands are recorded as an [`ExprKind::DialectOp`]
/// node, so the column reference is rendered together with the statement that
/// contains the predicate: it is qualified in a multi-table query, and an
/// aliased column keeps its `"alias"."column"` prefix.
fn column_binary_op<C: Column>(op: &'static str, val: &str) -> Expr<bool, C::Table> {
    Expr::from_kind(ExprKind::DialectOp {
        op,
        lhs: Box::new(C::expr_kind()),
        rhs: Box::new(ExprKind::Value(SqlVal::Text(val.to_owned()))),
    })
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
    use crate::dialect::Postgres;
    use rurqe_core::expr::render_expr;
    use rurqe_core::schema::{AliasedCol, TableRef};
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

    /// Renders `expr` under the PostgreSQL dialect against `relations`.
    fn render(expr: &ExprKind, relations: &[TableRef]) -> (String, Vec<SqlVal>) {
        let mut sql = String::new();
        let mut params = Vec::new();
        render_expr::<Postgres>(&mut sql, &mut params, expr, relations);
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
        assert_eq!(sql, "(\"content\" @@ to_tsquery($1))");
        assert_eq!(params, vec![SqlVal::Text("english & rust".to_owned())]);
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
        let (sql, params) = render(expr.kind(), &[TableRef::plain("docs")]);
        assert_eq!(sql, "(\"metadata\" ?| $1)");
        assert_eq!(params, vec![SqlVal::Text("{\"a\",\"b\\\"c\"}".to_owned())]);
    }
}
