//! PostgreSQL-specific expression extensions and text search functions.

use rurqe_core::expr::{
    BinaryOp, Expr, ExprKind, IntoExpr, MergeScope, Merged, NonAggregate, SqlArrayElement, SqlVal,
    ToSqlVal,
};
use rurqe_core::query::MakeNullable;
use rurqe_core::schema::Column;

/// A column value the PostgreSQL regex operators read.
///
/// The operators compare text, so they are offered for a text column and for its
/// optional form alike.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a PostgreSQL text value",
    label = "the regex operators read text",
    note = "declare the column as `String`, `Option<String>`, `&'static str`, or `Option<&'static str>`"
)]
pub trait PgTextValue {}

impl PgTextValue for String {}
impl PgTextValue for Option<String> {}
impl PgTextValue for &'static str {}
impl PgTextValue for Option<&'static str> {}

/// A column value the PostgreSQL full-text search operators read.
///
/// Full-text search matches a `tsquery` against text, so it is offered for the
/// same value kinds as the regex operators.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a PostgreSQL text value",
    label = "full-text search matches text",
    note = "declare the column as `String`, `Option<String>`, `&'static str`, or `Option<&'static str>`"
)]
pub trait PgTextSearchValue {}

impl PgTextSearchValue for String {}
impl PgTextSearchValue for Option<String> {}
impl PgTextSearchValue for &'static str {}
impl PgTextSearchValue for Option<&'static str> {}

/// A JSON or JSONB document carried as its text.
///
/// PostgreSQL prints a JSON document as text and accepts it in that form, so a
/// schema maps a `json` or `jsonb` column onto this wrapper without pulling in a
/// serialisation crate. The default type parameter makes a bare `PgJson` name a
/// document held as a `String`.
///
/// ```
/// use rurqe_postgres::rurqe_core::expr::{SqlVal, ToSqlVal};
/// use rurqe_postgres::PgJson;
///
/// assert_eq!(
///     PgJson("{\"a\": 1}").to_sql_val(),
///     SqlVal::Text("{\"a\": 1}".to_owned())
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PgJson<T = String>(pub T);

impl<T: Into<String>> ToSqlVal for PgJson<T> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Text(self.0.into())
    }
}

impl<T: Clone + Into<String>> ToSqlVal for &PgJson<T> {
    fn to_sql_val(self) -> SqlVal {
        SqlVal::Text(self.0.clone().into())
    }
}

impl<T> MakeNullable for PgJson<T> {
    type Nullable = Option<PgJson<T>>;
}

/// A column value the PostgreSQL JSON operators read.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a PostgreSQL JSON value",
    label = "the JSON operators read a JSON document",
    note = "declare the column as `PgJson`, `Option<PgJson>`, or — with the `serde_json` feature — as `serde_json::Value` or `Json<T>`"
)]
pub trait PgJsonValue {}

impl<T> PgJsonValue for PgJson<T> {}
impl<T> PgJsonValue for Option<PgJson<T>> {}

#[cfg(feature = "serde_json")]
impl PgJsonValue for serde_json::Value {}
#[cfg(feature = "serde_json")]
impl PgJsonValue for Option<serde_json::Value> {}
#[cfg(feature = "serde_json")]
impl<T> PgJsonValue for rurqe_core::expr::Json<T> {}
#[cfg(feature = "serde_json")]
impl<T> PgJsonValue for Option<rurqe_core::expr::Json<T>> {}

/// PostgreSQL POSIX regular expression operators.
///
/// Every operator keeps the column symbolic, so its reference is qualified
/// against the relations of the statement the predicate belongs to and quoted by
/// the dialect:
///
/// ```text
/// ("content" ~ $1)        regex_match
/// ("content" ~* $1)       regex_match_ci
/// ("content" !~ $1)       regex_not_match
/// ("content" !~* $1)      regex_not_match_ci
/// ```
///
/// A column of another value kind has no regex operators:
///
/// ```compile_fail
/// use rurqe_postgres::PgRegexExpressions;
///
/// rurqe_postgres::rurqe_core::table! {
///     Doc => "docs" {
///         DocRank => "rank": i64,
///     }
/// }
///
/// // Fails to compile: `rank` holds numbers, not text.
/// let _ = DocRank.regex_match("^1");
/// ```
pub trait PgRegexExpressions {
    /// The scope of the table the column belongs to.
    type Scope;

    /// POSIX regex match (`~`).
    fn regex_match(self, pattern: &str) -> Expr<bool, Self::Scope>;

    /// POSIX regex case-insensitive match (`~*`).
    fn regex_match_ci(self, pattern: &str) -> Expr<bool, Self::Scope>;

    /// POSIX regex does not match (`!~`).
    fn regex_not_match(self, pattern: &str) -> Expr<bool, Self::Scope>;

    /// POSIX regex case-insensitive does not match (`!~*`).
    fn regex_not_match_ci(self, pattern: &str) -> Expr<bool, Self::Scope>;
}

impl<C> PgRegexExpressions for C
where
    C: Column,
    C::RustType: PgTextValue,
{
    type Scope = C::Table;

    fn regex_match(self, pattern: &str) -> Expr<bool, C::Table> {
        column_binary_op::<C>("~", pattern)
    }

    fn regex_match_ci(self, pattern: &str) -> Expr<bool, C::Table> {
        column_binary_op::<C>("~*", pattern)
    }

    fn regex_not_match(self, pattern: &str) -> Expr<bool, C::Table> {
        column_binary_op::<C>("!~", pattern)
    }

    fn regex_not_match_ci(self, pattern: &str) -> Expr<bool, C::Table> {
        column_binary_op::<C>("!~*", pattern)
    }
}

/// PostgreSQL JSON and JSONB operators.
///
/// The operators read a JSON document, so they are offered for a column whose
/// Rust type is a JSON value ([`PgJsonValue`]). A column holding text is not a
/// JSON document:
///
/// ```compile_fail
/// use rurqe_postgres::PgJsonExpressions;
///
/// rurqe_postgres::rurqe_core::table! {
///     Doc => "docs" {
///         DocContent => "content": String,
///     }
/// }
///
/// // Fails to compile: `content` holds text, not a JSON document.
/// let _ = DocContent.json_contains("{\"a\": 1}");
/// ```
pub trait PgJsonExpressions {
    /// The scope of the table the column belongs to.
    type Scope;

    /// JSONB contains (`@>`).
    fn json_contains(self, other: &str) -> Expr<bool, Self::Scope>;

    /// JSONB exists / has key (`?`).
    fn json_has_key(self, key: &str) -> Expr<bool, Self::Scope>;

    /// JSONB has any keys (`?|`).
    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool, Self::Scope>;

    /// JSONB value at an object key or an array index (`->`).
    fn json_get<K: Into<JsonKey>>(self, key: K) -> Expr<String, Self::Scope>;

    /// JSONB value at an object key or an array index, rendered as text (`->>`).
    fn json_get_text<K: Into<JsonKey>>(self, key: K) -> Expr<String, Self::Scope>;
}

impl<C> PgJsonExpressions for C
where
    C: Column,
    C::RustType: PgJsonValue,
{
    type Scope = C::Table;

    fn json_contains(self, other: &str) -> Expr<bool, C::Table> {
        column_binary_op::<C>("@>", other)
    }

    fn json_has_key(self, key: &str) -> Expr<bool, C::Table> {
        column_binary_op::<C>("?", key)
    }

    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool, C::Table> {
        // `jsonb ?| text[]` reads an array operand, so every key binds as one
        // element of a single array parameter. A text literal would have the
        // driver describe the parameter as `text`, which PostgreSQL rejects for
        // the `text[]` operand of the operator.
        Expr::from_kind(ExprKind::DialectOp {
            op: "?|",
            lhs: Box::new(C::expr_kind()),
            rhs: Box::new(ExprKind::Value(SqlVal::Array(
                keys.iter()
                    .map(|key| SqlVal::Text((*key).to_owned()))
                    .collect(),
            ))),
        })
    }

    fn json_get<K: Into<JsonKey>>(self, key: K) -> Expr<String, C::Table> {
        json_extract::<C>("->", key.into())
    }

    fn json_get_text<K: Into<JsonKey>>(self, key: K) -> Expr<String, C::Table> {
        json_extract::<C>("->>", key.into())
    }
}

/// PostgreSQL full-text search operators.
///
/// Full-text search matches a `tsquery` against text, so a column of another
/// value kind has no such operator:
///
/// ```compile_fail
/// use rurqe_postgres::PgTextSearchExpressions;
///
/// rurqe_postgres::rurqe_core::table! {
///     Doc => "docs" {
///         DocRank => "rank": i64,
///     }
/// }
///
/// // Fails to compile: full-text search matches text.
/// let _ = DocRank.matches_tsquery("rust");
/// ```
pub trait PgTextSearchExpressions {
    /// The scope of the table the column belongs to.
    type Scope;

    /// Full-text search match (`@@ to_tsquery(?)`).
    fn matches_tsquery(self, query: &str) -> Expr<bool, Self::Scope>;
}

impl<C> PgTextSearchExpressions for C
where
    C: Column,
    C::RustType: PgTextSearchValue,
{
    type Scope = C::Table;

    fn matches_tsquery(self, query: &str) -> Expr<bool, C::Table> {
        Expr::from_kind(ExprKind::DialectOp {
            op: "@@",
            lhs: Box::new(C::expr_kind()),
            rhs: Box::new(to_tsquery(query).into_kind()),
        })
    }
}

/// The path component a JSONB extraction operator addresses.
///
/// `->` and `->>` address an object member by its key and an array element by its
/// zero-based index, so both operators accept either form where a path component
/// is written: `metadata.json_get("owner")` reads an object member, while
/// `metadata.json_get(0i32)` reads the first element of an array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonKey {
    /// An object member addressed by its key.
    Key(String),
    /// An array element addressed by its zero-based index.
    Index(i32),
}

impl From<&str> for JsonKey {
    fn from(key: &str) -> Self {
        JsonKey::Key(key.to_owned())
    }
}

impl From<String> for JsonKey {
    fn from(key: String) -> Self {
        JsonKey::Key(key)
    }
}

impl From<i32> for JsonKey {
    fn from(index: i32) -> Self {
        JsonKey::Index(index)
    }
}

impl JsonKey {
    /// The operand the extraction operator reads the path component from.
    fn to_operand(&self) -> ExprKind {
        match self {
            // An object key binds as a parameter, which the server types as the
            // `text` operand of the operator.
            JsonKey::Key(key) => ExprKind::Value(SqlVal::Text(key.clone())),
            // An array index is written into the statement as an integer literal.
            // The statement is sent without parameter type hints, so an untyped
            // parameter of `->` / `->>` is resolved against both the `text` and
            // the `integer` overload of the operator, and the server types the
            // unknown argument as `text`. The adapter then refuses to encode the
            // `i32` index for a text column, so the extraction aborts at bind
            // time. A literal carries its `integer` type to the server, which
            // selects the `integer` overload before any parameter is encoded.
            // The literal is built from an `i32`, so it holds nothing but digits
            // and a sign.
            JsonKey::Index(index) => ExprKind::Raw {
                fragments: vec![index.to_string()],
                params: Vec::new(),
            },
        }
    }
}

/// Builds a `(<column> -> <path>)` extraction over the JSONB column `C`.
///
/// The column stays symbolic, so its reference is qualified against the relations
/// of the statement the extraction belongs to and quoted by the dialect. An object
/// key is read from a bound parameter, while an array index is carried as an
/// integer literal of the statement, so the server picks the `integer` overload of
/// the operator instead of typing a parameter as `text`.
fn json_extract<C: Column>(op: &'static str, key: JsonKey) -> Expr<String, C::Table> {
    Expr::from_kind(ExprKind::DialectOp {
        op,
        lhs: Box::new(C::expr_kind()),
        rhs: Box::new(key.to_operand()),
    })
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
///
/// The operators keep the dialect marker of their right-hand operand, so an
/// expression or a subquery built for another engine is refused by the statement
/// the predicate belongs to:
///
/// ```compile_fail
/// use rurqe_postgres::PgArrayExpressions;
/// use rurqe_postgres::rurqe_core::dialect::Dialect;
/// use rurqe_postgres::rurqe_core::expr::{Expr, ExprKind, NonAggregate, SqlVal};
/// use rurqe_postgres::rurqe_core::query::SelectQuery;
///
/// struct OtherDialect;
///
/// impl Dialect for OtherDialect {
///     fn push_quoted_identifier(out: &mut String, ident: &str) {
///         out.push_str(ident);
///     }
///
///     fn push_placeholder(out: &mut String, index: usize) {
///         let _ = (out, index);
///     }
/// }
///
/// rurqe_postgres::rurqe_core::table! {
///     Post => "posts" {
///         PostTags => "tags": Vec<String>,
///     }
/// }
///
/// let operand = Expr::<String, (), NonAggregate, OtherDialect>::from_kind(
///     ExprKind::Value(SqlVal::Text("rust".to_owned())),
/// );
///
/// // Fails to compile: the operand was built for another engine, so the dialect
/// // marker it carries is refused by the PostgreSQL statement.
/// let _ = SelectQuery::<rurqe_postgres::Postgres>::new()
///     .from::<Post>()
///     .select((PostTags,))
///     .where_(PostTags.any(operand));
/// ```
pub trait PgArrayExpressions<T> {
    /// The scope of the table the array column belongs to.
    type Scope;

    /// Matches arrays containing every element of `other` (`@>`).
    #[allow(clippy::type_complexity)]
    fn array_contains<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays whose every element is contained in `other` (`<@`).
    #[allow(clippy::type_complexity)]
    fn array_contained_by<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays sharing at least one element with `other` (`&&`).
    #[allow(clippy::type_complexity)]
    fn array_overlaps<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays holding an element equal to `value` (`<value> = ANY(...)`).
    #[allow(clippy::type_complexity)]
    fn any<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>;

    /// Matches arrays whose every element equals `value` (`<value> = ALL(...)`).
    #[allow(clippy::type_complexity)]
    fn all<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
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

    #[allow(clippy::type_complexity)]
    fn array_contains<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::from_kind(array_operator("@>", S::expr_kind(), other.into_expr_kind()))
    }

    #[allow(clippy::type_complexity)]
    fn array_contained_by<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::from_kind(array_operator("<@", S::expr_kind(), other.into_expr_kind()))
    }

    #[allow(clippy::type_complexity)]
    fn array_overlaps<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::from_kind(array_operator("&&", S::expr_kind(), other.into_expr_kind()))
    }

    #[allow(clippy::type_complexity)]
    fn any<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
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

    #[allow(clippy::type_complexity)]
    fn all<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, V::Dialect>
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
    use super::{
        plainto_tsquery, to_tsquery, to_tsvector, JsonKey, PgArrayExpressions, PgJson,
        PgJsonExpressions, PgJsonValue, PgRegexExpressions, PgTextSearchExpressions,
        PgTextSearchValue, PgTextValue,
    };
    use crate::dialect::Postgres;
    use rurqe_core::expr::{render_expr, Expr, ExprKind, NonAggregate, SqlVal, ToSqlVal};
    use rurqe_core::query::MakeNullable;
    use rurqe_core::schema::{AliasedCol, Column, TableRef};
    use rurqe_core::table;

    table! {
        Doc => "docs" {
            DocContent => "content": String,
            DocMetadata => "metadata": PgJson,
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
        let tag = Expr::<String, (), NonAggregate, Postgres>::from_kind(ExprKind::Value(
            SqlVal::Text("rust".to_owned()),
        ));
        let score = Expr::<i64, (), NonAggregate, Postgres>::from_kind(ExprKind::Value(
            SqlVal::I64(0),
        ));

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
        let value: Expr<String, Doc> = DocMetadata.json_get("owner");
        let (sql, _) = render(
            value.kind(),
            &[TableRef::plain("docs"), TableRef::plain("posts")],
        );

        assert_eq!(sql, "(\"docs\".\"metadata\" -> $1)");
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
        fn assert_nullable<T: MakeNullable<Nullable = Option<T>>>() {}

        assert_json_value::<PgJson>();
        assert_json_value::<PgJson<String>>();
        assert_json_value::<Option<PgJson>>();

        #[cfg(feature = "serde_json")]
        {
            assert_json_value::<serde_json::Value>();
            assert_json_value::<Option<serde_json::Value>>();
            assert_json_value::<rurqe_core::expr::Json<String>>();
            assert_json_value::<Option<rurqe_core::expr::Json<String>>>();
        }

        assert_nullable::<PgJson>();
        assert_eq!(
            PgJson("{\"a\": 1}").to_sql_val(),
            SqlVal::Text("{\"a\": 1}".to_owned())
        );
        assert_eq!((&PgJson("{}")).to_sql_val(), SqlVal::Text("{}".to_owned()));
    }

    #[test]
    fn a_json_column_is_never_read_as_text() {
        fn assert_json_column<C: Column>()
        where
            C::RustType: PgJsonValue,
        {
        }

        assert_json_column::<DocMetadata>();
    }
}
