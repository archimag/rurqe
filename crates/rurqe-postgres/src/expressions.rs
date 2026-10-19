//! PostgreSQL-specific expression extensions and text search functions.

use rurqe_core::expr::{Expr, SqlVal};
use rurqe_core::schema::Column;

/// PostgreSQL-specific expressions and operators.
pub trait PgExpressions {
    /// POSIX regex match (`~`).
    fn regex_match(self, pattern: &str) -> Expr<bool>;

    /// POSIX regex case-insensitive match (`~*`).
    fn regex_match_ci(self, pattern: &str) -> Expr<bool>;

    /// POSIX regex does not match (`!~`).
    fn regex_not_match(self, pattern: &str) -> Expr<bool>;

    /// POSIX regex case-insensitive does not match (`!~*`).
    fn regex_not_match_ci(self, pattern: &str) -> Expr<bool>;

    /// JSONB contains (`@>`).
    fn json_contains(self, other: &str) -> Expr<bool>;

    /// JSONB exists / has key (`?`).
    fn json_has_key(self, key: &str) -> Expr<bool>;

    /// JSONB has any keys (`?|`).
    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool>;

    /// Full-text search match (`@@ to_tsquery(?)`).
    fn matches_tsquery(self, query: &str) -> Expr<bool>;
}

impl<C: Column> PgExpressions for C {
    fn regex_match(self, pattern: &str) -> Expr<bool> {
        raw_binary_op(C::NAME, "~", pattern)
    }

    fn regex_match_ci(self, pattern: &str) -> Expr<bool> {
        raw_binary_op(C::NAME, "~*", pattern)
    }

    fn regex_not_match(self, pattern: &str) -> Expr<bool> {
        raw_binary_op(C::NAME, "!~", pattern)
    }

    fn regex_not_match_ci(self, pattern: &str) -> Expr<bool> {
        raw_binary_op(C::NAME, "!~*", pattern)
    }

    fn json_contains(self, other: &str) -> Expr<bool> {
        raw_binary_op(C::NAME, "@>", other)
    }

    fn json_has_key(self, key: &str) -> Expr<bool> {
        raw_binary_op(C::NAME, "?", key)
    }

    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool> {
        let formatted = format!("{{{}}}", keys.join(","));
        raw_binary_op(C::NAME, "?|", &formatted)
    }

    fn matches_tsquery(self, query: &str) -> Expr<bool> {
        Expr::raw_fragments(
            vec![format!("(\"{}\" @@ to_tsquery(", C::NAME), "))".to_owned()],
            vec![SqlVal::Text(query.to_owned())],
        )
    }
}

fn raw_binary_op(col: &str, op: &str, val: &str) -> Expr<bool> {
    Expr::raw_fragments(
        vec![format!("(\"{col}\" {op} "), ")".to_owned()],
        vec![SqlVal::Text(val.to_owned())],
    )
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
    use rurqe_core::table;

    table! {
        Doc => "docs" {
            DocContent => "content": String,
            DocMetadata => "metadata": String,
        }
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
    fn text_search_functions_build_raw_ast() {
        let v = to_tsvector("fat cats");
        assert!(matches!(v.kind(), ExprKind::Raw { .. }));

        let q = to_tsquery("cat & rat");
        assert!(matches!(q.kind(), ExprKind::Raw { .. }));

        let p = plainto_tsquery("the fat cat");
        assert!(matches!(p.kind(), ExprKind::Raw { .. }));
    }
}
