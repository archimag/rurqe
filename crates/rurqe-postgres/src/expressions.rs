//! PostgreSQL-specific expression extensions and text search functions.

use rurqe_core::expr::{
    BinaryOp, Expr, ExprKind, IntoExpr, MergeScope, Merged, SqlArrayElement, SqlVal,
};
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

/// The array value a column holds, together with the element type it carries.
///
/// The trait covers the plain and the nullable array column alike, so the
/// operators of [`PgArrayExpressions`] are declared once for both of them.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an array value",
    label = "the array operators require a column holding `Vec<T>` or `Option<Vec<T>>`",
    note = "declare the column with an array Rust type and introspect it from a PostgreSQL `T[]` column"
)]
pub trait PgArrayElements<T> {}

impl<T: SqlArrayElement> PgArrayElements<T> for Vec<T> {}

impl<T: SqlArrayElement> PgArrayElements<T> for Option<Vec<T>> {}

/// PostgreSQL array operators applied to a column holding an array.
///
/// Every operator keeps the column symbolic, so its reference is qualified
/// against the relations of the statement the predicate belongs to and quoted by
/// the dialect. The operators map onto the native array support of PostgreSQL:
///
/// ```text
/// ("tags" @> $1)          array_contains
/// ("tags" <@ $1)          array_contained_by
/// ("tags" && $1)          array_overlaps
/// ($1 = ANY("tags"))      any
/// ($1 = ALL("tags"))      all
/// ```
///
/// The element type is inferred from the column, so the operators apply both to
/// a column declared as `Vec<T>` and to a nullable one declared as
/// `Option<Vec<T>>` without an extra annotation.
pub trait PgArrayExpressions<T> {
    /// The scope of the table the array column belongs to.
    type Scope;

    /// Matches arrays containing every element of `other` (`@>`).
    fn array_contains<V>(self, other: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays whose every element is contained in `other` (`<@`).
    fn array_contained_by<V>(self, other: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays sharing at least one element with `other` (`&&`).
    fn array_overlaps<V>(self, other: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays holding an element equal to `value` (`<value> = ANY(...)`).
    fn any<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>;

    /// Matches arrays whose every element equals `value` (`<value> = ALL(...)`).
    fn all<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>;
}

impl<T, S> PgArrayExpressions<T> for S
where
    S: Column,
    S::RustType: PgArrayElements<T>,
{
    type Scope = S::Table;

    fn array_contains<V>(self, other: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::from_kind(array_operator("@>", S::expr_kind(), other.into_expr_kind()))
    }

    fn array_contained_by<V>(self, other: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::from_kind(array_operator("<@", S::expr_kind(), other.into_expr_kind()))
    }

    fn array_overlaps<V>(self, other: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::from_kind(array_operator("&&", S::expr_kind(), other.into_expr_kind()))
    }

    fn any<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        Expr::from_kind(array_quantifier(
            "ANY",
            S::expr_kind(),
            value.into_expr_kind(),
        ))
    }

    fn all<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        Expr::from_kind(array_quantifier(
            "ALL",
            S::expr_kind(),
            value.into_expr_kind(),
        ))
    }
}

/// Builds a `(<array> <op> <other>)` predicate over the array expression `array`.
fn array_operator(op: &'static str, array: ExprKind, other: ExprKind) -> ExprKind {
    ExprKind::DialectOp {
        op,
        lhs: Box::new(array),
        rhs: Box::new(other),
    }
}

/// Builds a `(<value> = ANY|ALL (<array>))` predicate over the array `array`.
///
/// `ANY` and `ALL` are written as the function call PostgreSQL spells them, so
/// the array still renders its column reference through the dialect instead of
/// reaching the buffer as pre-rendered text.
fn array_quantifier(quantifier: &'static str, array: ExprKind, value: ExprKind) -> ExprKind {
    ExprKind::Binary {
        op: BinaryOp::Eq,
        lhs: Box::new(value),
        rhs: Box::new(ExprKind::Function {
            name: quantifier,
            args: vec![array],
        }),
    }
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

    table! {
        Post => "posts" {
            PostTags => "tags": Vec<String>,
            PostScores => "scores": Option<Vec<i64>>,
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
}
