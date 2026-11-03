//! PostgreSQL-specific expression extensions and text search functions.

use rurqe_core::expr::{
    Expr, ExprKind, IntoExpr, MergeDialect, MergeScope, Merged, MergedDialect, NonAggregate,
    SqlArrayElement, SqlVal, ToSqlVal,
};
use rurqe_core::query::MakeNullable;
use rurqe_core::row::{FromSqlValue, RowDecodeError};
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
/// Full-text search compares a `tsquery` against a `tsvector` document, so the
/// column supplies the text the document is built from: it is offered for the
/// same value kinds as the regex operators, and the predicate converts it with
/// `to_tsvector` before the comparison. PostgreSQL defines `@@` only between a
/// `tsvector` and a `tsquery`, so the document is built from the column itself
/// and the predicate renders `(to_tsvector(<column>) @@ to_tsquery($1))`.
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

/// A `json` document carried as its text.
///
/// PostgreSQL prints a `json` document as text and accepts it in that form, so a
/// schema maps a `json` column onto this wrapper without pulling in a
/// serialisation crate. The default type parameter makes a bare `PgJson` name a
/// document held as a `String`.
///
/// A `json` column offers the extraction operators of [`PgJsonExpressions`] and
/// nothing else: containment and the existence tests are defined for `jsonb`
/// alone, so a column of this kind is declared as [`PgJsonb`] to reach them.
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

/// A `jsonb` document carried as its text.
///
/// The wrapper mirrors [`PgJson`] and adds the operators PostgreSQL defines for
/// `jsonb` alone: containment (`@>`) and the existence tests (`?`, `?|`).
///
/// ```
/// use rurqe_postgres::rurqe_core::expr::{SqlVal, ToSqlVal};
/// use rurqe_postgres::PgJsonb;
///
/// assert_eq!(
///     PgJsonb("[1, 2]").to_sql_val(),
///     SqlVal::Text("[1, 2]".to_owned())
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PgJsonb<T = String>(pub T);

/// Generates the conversions a document wrapper shares with its sibling.
///
/// A `json` document and a `jsonb` document travel as the text of the document,
/// so the conversions of [`PgJson`] and [`PgJsonb`] differ in nothing but the
/// type they are implemented for.
macro_rules! impl_pg_document {
    ($document:ident) => {
        impl<T: Into<String>> ToSqlVal for $document<T> {
            fn to_sql_val(self) -> SqlVal {
                SqlVal::Text(self.0.into())
            }
        }

        impl<T: Clone + Into<String>> ToSqlVal for &$document<T> {
            fn to_sql_val(self) -> SqlVal {
                SqlVal::Text(self.0.clone().into())
            }
        }

        impl<T> MakeNullable for $document<T> {
            type Nullable = Option<$document<T>>;
        }

        impl<T: 'static + Send + Sync> PgJsonValue for $document<T> {
            type Document = $document<T>;
        }

        impl<T: From<String> + 'static + Send + Sync> FromSqlValue for $document<T> {
            fn from_sql_value(value: &SqlVal) -> Result<Self, RowDecodeError> {
                document_text::<$document<T>>(value).map(|text| $document(T::from(text)))
            }
        }
    };
}

impl_pg_document!(PgJson);
impl_pg_document!(PgJsonb);

/// Reads the text of a JSON document out of the value a column reports.
///
/// A driver decodes a `json` or `jsonb` column into the text of the document,
/// and the `serde_json` binding decodes it into a `SqlVal::Json` value; both
/// forms carry the same document, so both are read back into the text the
/// wrapper holds.
fn document_text<D>(value: &SqlVal) -> Result<String, RowDecodeError> {
    match value {
        SqlVal::Text(text) => Ok(text.clone()),
        #[cfg(feature = "serde_json")]
        SqlVal::Json(json) => Ok(json.to_string()),
        _ => Err(RowDecodeError::TypeMismatch {
            target: std::any::type_name::<D>(),
            found: value.variant_name(),
        }),
    }
}

/// A column value the PostgreSQL JSON extraction operators read.
///
/// Extraction (`->` and `->>`) is defined for a `json` document and a `jsonb`
/// document alike, so both storage kinds satisfy the marker. The kind still
/// decides the type of the extracted value, which is why the marker names the
/// [`Document`](PgJsonValue::Document) an extraction yields.
///
/// A document type that does not state its storage kind — `serde_json::Value`
/// and `Json<T>` — satisfies the marker as well, because extraction is valid for
/// either kind; the operators defined for `jsonb` alone are offered through
/// [`PgJsonbValue`] instead.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a PostgreSQL JSON value",
    label = "the JSON operators read a JSON document",
    note = "declare the column as `PgJson`, `PgJsonb`, `Option<PgJson>`, or `Option<PgJsonb>`, or — with the `serde_json` feature — as `serde_json::Value` or `Json<T>`"
)]
pub trait PgJsonValue {
    /// The document type an extraction (`->`) from the value yields.
    type Document: 'static + Send + Sync;
}

/// A column value the JSONB-only PostgreSQL operators read.
///
/// Containment (`@>`) and the existence tests (`?`, `?|`) are defined for a
/// `jsonb` document alone. The marker is implemented for the values that state
/// that storage — [`PgJsonb`] and its optional form — and never for [`PgJson`]
/// or for a document type that does not state its kind.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not state a `jsonb` document",
    label = "the containment and existence operators read `jsonb` alone",
    note = "declare the column as `PgJsonb` or `Option<PgJsonb>`; a `PgJson` column and a document type without a storage kind offer the extraction operators only"
)]
pub trait PgJsonbValue: PgJsonValue {}

impl<T: 'static + Send + Sync> PgJsonValue for Option<PgJson<T>> {
    type Document = PgJson<T>;
}

impl<T: 'static + Send + Sync> PgJsonValue for Option<PgJsonb<T>> {
    type Document = PgJsonb<T>;
}

impl<T: 'static + Send + Sync> PgJsonbValue for PgJsonb<T> {}

impl<T: 'static + Send + Sync> PgJsonbValue for Option<PgJsonb<T>> {}

#[cfg(feature = "serde_json")]
impl PgJsonValue for serde_json::Value {
    type Document = serde_json::Value;
}

#[cfg(feature = "serde_json")]
impl PgJsonValue for Option<serde_json::Value> {
    type Document = serde_json::Value;
}

#[cfg(feature = "serde_json")]
impl<T: 'static + Send + Sync> PgJsonValue for rurqe_core::expr::Json<T> {
    type Document = rurqe_core::expr::Json<T>;
}

#[cfg(feature = "serde_json")]
impl<T: 'static + Send + Sync> PgJsonValue for Option<rurqe_core::expr::Json<T>> {
    type Document = rurqe_core::expr::Json<T>;
}

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
/// rurqe_postgres::rurqe_core::table!(
///     Doc => "docs" {
///         DocRank => "rank": i64,
///     }
/// );
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

/// PostgreSQL JSON extraction operators.
///
/// Extraction is defined for a `json` document and a `jsonb` document alike, so
/// the operators are offered for a column whose Rust type is a JSON value
/// ([`PgJsonValue`]). A column holding text is not a JSON document:
///
/// ```compile_fail
/// use rurqe_postgres::PgJsonExpressions;
///
/// rurqe_postgres::rurqe_core::table!(
///     Doc => "docs" {
///         DocContent => "content": String,
///     }
/// );
///
/// // Fails to compile: `content` holds text, not a JSON document.
/// let _ = DocContent.json_get("owner");
/// ```
///
/// The kind of the column decides the document an extraction yields: `->`
/// returns a `json` value for a `json` column and a `jsonb` value for a `jsonb`
/// one, so [`json_get`](PgJsonExpressions::json_get) carries the document type of
/// the column instead of text. Only
/// [`json_get_text`](PgJsonExpressions::json_get_text) — the `->>` operator —
/// yields text.
pub trait PgJsonExpressions {
    /// The scope of the table the column belongs to.
    type Scope;

    /// The document type an extraction from the column yields.
    type Document: 'static + Send + Sync;

    /// The document at an object key or an array index (`->`).
    fn json_get<K: Into<JsonKey>>(self, key: K) -> Expr<Self::Document, Self::Scope>;

    /// The value at an object key or an array index, rendered as text (`->>`).
    fn json_get_text<K: Into<JsonKey>>(self, key: K) -> Expr<String, Self::Scope>;
}

impl<C> PgJsonExpressions for C
where
    C: Column,
    C::RustType: PgJsonValue,
{
    type Scope = C::Table;
    type Document = <C::RustType as PgJsonValue>::Document;

    fn json_get<K: Into<JsonKey>>(self, key: K) -> Expr<Self::Document, C::Table> {
        json_extract::<C, Self::Document>("->", key.into())
    }

    fn json_get_text<K: Into<JsonKey>>(self, key: K) -> Expr<String, C::Table> {
        json_extract::<C, String>("->>", key.into())
    }
}

/// PostgreSQL JSONB operators.
///
/// Containment (`@>`) and the existence tests (`?`, `?|`) are defined for a
/// `jsonb` document alone: the `json` type has no such operators, and the server
/// rejects a statement that applies one to it. The operators are therefore
/// offered for a column whose Rust type states a `jsonb` storage
/// ([`PgJsonbValue`]): a [`PgJsonb`] column, or its optional form.
///
/// A [`PgJson`] column holds the other kind, and a document type that states no
/// kind holds either, so both offer the extraction operators alone:
///
/// ```compile_fail
/// use rurqe_postgres::{PgJson, PgJsonbExpressions};
///
/// rurqe_postgres::rurqe_core::table!(
///     Doc => "docs" {
///         DocMetadata => "metadata": PgJson,
///     }
/// );
///
/// // Fails to compile: containment is defined for `jsonb` alone, and the column
/// // states a `json` document.
/// let _ = DocMetadata.json_contains("{\"a\": 1}");
/// ```
///
/// A column holding plain text is not a JSON document either:
///
/// ```compile_fail
/// use rurqe_postgres::PgJsonbExpressions;
///
/// rurqe_postgres::rurqe_core::table!(
///     Doc => "docs" {
///         DocContent => "content": String,
///     }
/// );
///
/// // Fails to compile: `content` holds text, not a JSON document.
/// let _ = DocContent.json_contains("{\"a\": 1}");
/// ```
pub trait PgJsonbExpressions {
    /// The scope of the table the column belongs to.
    type Scope;

    /// JSONB contains (`@>`).
    fn json_contains(self, other: &str) -> Expr<bool, Self::Scope>;

    /// JSONB exists / has key (`?`).
    fn json_has_key(self, key: &str) -> Expr<bool, Self::Scope>;

    /// JSONB has any keys (`?|`).
    fn json_has_any_keys(self, keys: &[&str]) -> Expr<bool, Self::Scope>;
}

impl<C> PgJsonbExpressions for C
where
    C: Column,
    C::RustType: PgJsonbValue,
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
        // the `text[]` operand of the operator. The array references no
        // relation, so the literal carries the empty scope and the operator
        // inherits the scope of the column alone.
        let array: Expr<Vec<SqlVal>> = Expr::from_kind(ExprKind::Value(SqlVal::Array(
            keys.iter()
                .map(|key| SqlVal::Text((*key).to_owned()))
                .collect(),
        )));

        Expr::column::<C>().dialect_op::<Vec<SqlVal>, bool, _>("?|", array)
    }
}

/// PostgreSQL full-text search operators.
///
/// A full-text search compares a `tsquery` against a `tsvector` document, so the
/// text column is converted into the document it is searched as:
///
/// ```text
/// (to_tsvector("content") @@ to_tsquery($1))     matches_tsquery
/// ```
///
/// PostgreSQL defines `@@` only between a `tsvector` and a `tsquery`, so a bare
/// text operand would render `text @@ tsquery` and the server would reject the
/// statement with `operator does not exist: text @@ tsquery`. The document is
/// built from the symbolic reference of the column, so the enclosing statement
/// still qualifies it against its relations and the dialect still quotes the
/// identifiers.
///
/// A column of another value kind has no such operator:
///
/// ```compile_fail
/// use rurqe_postgres::PgTextSearchExpressions;
///
/// rurqe_postgres::rurqe_core::table!(
///     Doc => "docs" {
///         DocRank => "rank": i64,
///     }
/// );
///
/// // Fails to compile: full-text search matches text.
/// let _ = DocRank.matches_tsquery("rust");
/// ```
pub trait PgTextSearchExpressions {
    /// The scope of the table the column belongs to.
    type Scope;

    /// Full-text search match (`to_tsvector(<column>) @@ to_tsquery(?)`).
    fn matches_tsquery(self, query: &str) -> Expr<bool, Self::Scope>;
}

impl<C> PgTextSearchExpressions for C
where
    C: Column,
    C::RustType: PgTextSearchValue,
{
    type Scope = C::Table;

    fn matches_tsquery(self, query: &str) -> Expr<bool, C::Table> {
        // `@@` relates a `tsvector` document to a `tsquery`, so the text column
        // is converted into the document it is searched as. The call keeps the
        // column reference symbolic: the enclosing statement qualifies it
        // against its relations, an aliased column keeps its alias prefix, and
        // the dialect quotes the identifiers. The predicate inherits the scope
        // of the searched column, so it references the table of that column and
        // nothing else.
        Expr::column_function::<C>("to_tsvector")
            .dialect_op::<String, bool, _>("@@", to_tsquery(query))
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

/// Builds a `(<column> -> <path>)` extraction over the JSON column `C`.
///
/// The column stays symbolic, so its reference is qualified against the relations
/// of the statement the extraction belongs to and quoted by the dialect. An object
/// key is read from a bound parameter, while an array index is carried as an
/// integer literal of the statement, so the server picks the `integer` overload of
/// the operator instead of typing a parameter as `text`. The path component
/// references no relation, so it carries the empty scope, and the extraction
/// inherits the scope of the column alone.
///
/// `Out` is the document the operator yields: `->` returns a value of the column's
/// own storage kind, so the caller names the kind it extracts, while `->>` names
/// `String`.
fn json_extract<C, Out>(op: &'static str, key: JsonKey) -> Expr<Out, C::Table>
where
    C: Column,
    C::RustType: PgJsonValue,
{
    let column = Expr::column::<C>();

    match key {
        // An object key binds as a parameter, which the server types as the
        // `text` operand of the operator.
        JsonKey::Key(key) => {
            let path: Expr<String> = Expr::literal(key);
            column.dialect_op::<String, Out, _>(op, path)
        }
        // An array index is written into the statement as an integer literal.
        // The statement is sent without parameter type hints, so an untyped
        // parameter of `->` / `->>` is resolved against both the `text` and the
        // `integer` overload of the operator, and the server types the unknown
        // argument as `text`. The adapter then refuses to encode the `i32` index
        // for a text column, so the extraction aborts at bind time. A literal
        // carries its `integer` type to the server, which selects the `integer`
        // overload before any parameter is encoded. The literal is built from an
        // `i32`, so it holds nothing but digits and a sign.
        JsonKey::Index(index) => {
            let path: Expr<i32> = Expr::raw_owned_fragments(vec![index.to_string()], Vec::new());
            column.dialect_op::<i32, Out, _>(op, path)
        }
    }
}

/// Builds a `(<column> <op> <value>)` predicate over the column `C`.
///
/// The operator is recorded as an [`ExprKind::DialectOp`] node whose left
/// operand is the column itself, so the column reference is rendered together
/// with the statement that contains the predicate: it is qualified in a
/// multi-table query, and an aliased column keeps its `"alias"."column"`
/// prefix. The node inherits the scope of the column, so the predicate
/// references exactly the table of `C`, while the bound value it is compared
/// with references no relation.
fn column_binary_op<C: Column>(op: &'static str, val: &str) -> Expr<bool, C::Table> {
    Expr::column::<C>().dialect_op::<String, bool, _>(op, val.to_owned())
}

/// Builds a `to_tsvector(?)` expression.
pub fn to_tsvector(text: &str) -> Expr<String> {
    let params = vec![SqlVal::Text(text.to_owned())];
    Expr::raw_fragments(&["to_tsvector(", ")"], params)
}

/// Builds a `to_tsquery(?)` expression.
pub fn to_tsquery(query: &str) -> Expr<String> {
    let params = vec![SqlVal::Text(query.to_owned())];
    Expr::raw_fragments(&["to_tsquery(", ")"], params)
}

/// Builds a `plainto_tsquery(?)` expression.
pub fn plainto_tsquery(query: &str) -> Expr<String> {
    let params = vec![SqlVal::Text(query.to_owned())];
    Expr::raw_fragments(&["plainto_tsquery(", ")"], params)
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
/// The operators inherit the scope of the array column and merge it with the
/// scope of their operand, so a predicate comparing the array of one relation
/// with a value of another references both. The dialect marker names that same
/// merge of the dialect-neutral column with the operand's dialect, which reduces
/// to the operand's dialect for every operand.
///
/// An expression or a subquery built for another engine carries a dialect the
/// statement cannot render, so it is refused by the statement the predicate
/// belongs to:
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
/// rurqe_postgres::rurqe_core::table!(
///     Post => "posts" {
///         PostTags => "tags": Vec<String>,
///     }
/// );
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
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays whose every element is contained in `other` (`<@`).
    #[allow(clippy::type_complexity)]
    fn array_contained_by<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays sharing at least one element with `other` (`&&`).
    #[allow(clippy::type_complexity)]
    fn array_overlaps<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<Vec<T>>;

    /// Matches arrays holding an element equal to `value` (`<value> = ANY(...)`).
    #[allow(clippy::type_complexity)]
    fn any<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<T>;

    /// Matches arrays whose every element equals `value` (`<value> = ALL(...)`).
    #[allow(clippy::type_complexity)]
    fn all<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
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
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::column::<S>().dialect_op::<Vec<T>, bool, V>("@>", other)
    }

    #[allow(clippy::type_complexity)]
    fn array_contained_by<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::column::<S>().dialect_op::<Vec<T>, bool, V>("<@", other)
    }

    #[allow(clippy::type_complexity)]
    fn array_overlaps<V>(
        self,
        other: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<Vec<T>>,
    {
        Expr::column::<S>().dialect_op::<Vec<T>, bool, V>("&&", other)
    }

    #[allow(clippy::type_complexity)]
    fn any<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        Expr::column::<S>().dialect_quantified::<T, V>("ANY", value)
    }

    #[allow(clippy::type_complexity)]
    fn all<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Self::Scope, V::Scope>, NonAggregate, MergedDialect<(), V::Dialect>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        (): MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        Expr::column::<S>().dialect_quantified::<T, V>("ALL", value)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        plainto_tsquery, to_tsquery, to_tsvector, JsonKey, PgArrayExpressions, PgJson, PgJsonb,
        PgJsonbExpressions, PgJsonbValue, PgJsonExpressions, PgJsonValue, PgRegexExpressions,
        PgTextSearchExpressions, PgTextSearchValue, PgTextValue,
    };
    use crate::dialect::Postgres;
    use rurqe_core::expr::{render_expr, Expr, ExprKind, NonAggregate, SqlVal, ToSqlVal};
    use rurqe_core::query::MakeNullable;
    use rurqe_core::row::{FromSqlValue, RowDecodeError};
    use rurqe_core::schema::{AliasedCol, Column, TableRef};
    use rurqe_core::table;

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
        assert_eq!(qualified, "(to_tsvector(\"docs\".\"content\") @@ to_tsquery($1))");
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
        let tag = Expr::<String, (), NonAggregate, Postgres>::from_kind(ExprKind::Value(
            SqlVal::Text("rust".to_owned()),
        ));
        let score =
            Expr::<i64, (), NonAggregate, Postgres>::from_kind(ExprKind::Value(SqlVal::I64(0)));

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
        assert_eq!(
            (&PgJsonb("{}")).to_sql_val(),
            SqlVal::Text("{}".to_owned())
        );
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
}
