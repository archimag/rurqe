//! Procedural macros for RURQE query builder and relational execution engine.

mod dto;
mod embed_migrations;
mod parse;
mod raw_sql;
mod record;
mod select;
mod sql_type;

use proc_macro::TokenStream;

/// Builds an ad-hoc named-field projection for `SELECT` queries.
///
/// Every projection names a field of the produced record, so callers never index
/// rows by tuple position. A schema column marker (`UserId`, `users::EMAIL`)
/// derives its field from the column name and projects through the same column
/// rendering as a tuple selection, which qualifies the reference whenever the
/// statement spans several relations, while a computed projection keeps its own
/// type and is named explicitly:
///
/// ```ignore
/// let query = SelectQuery::<Postgres>::new()
///     .from::<User>()
///     .select(select!(
///         UserId,
///         UserEmail as contact_email,
///         order_count: Expr::count_all(),
///     ));
/// ```
///
/// The synthesized record is generic over its field types, so a `LEFT JOIN`ed
/// table lifts its projected fields into `Option<T>` at the call site. The
/// selection can also start a query directly with `select!(..).from(table)`.
///
/// An outer attribute written on a projection is carried onto the record field of
/// the same name, exactly as `dto!` carries the attributes of its declaration, so
/// a `#[doc]` comment or a lint level describes the field it was written for. An
/// attribute that compiles the field away (`#[cfg]`) is not supported: the
/// projection list fixes the row arity and the nested-row keys of the record, so
/// the statement would keep projecting a column its record no longer has a field
/// to decode into.
///
/// The trailing `as <alias>` form names the field and is never read as a Rust
/// cast. An alias spelled as a type (`Expr::count_all() as i64`, `users.id as
/// Text`) is refused instead of silently dropping the conversion and naming the
/// record field after a type; the leading form names the same field without the
/// ambiguity, and a conversion belongs in the expression itself:
///
/// ```ignore
/// let query = SelectQuery::<Postgres>::new()
///     .from::<Order>()
///     .select(select!(
///         order_total: OrderTotal,
///         widened: Expr::column::<OrderTotal>().cast::<f64>(),
///     ));
/// ```
#[proc_macro]
pub fn select(input: TokenStream) -> TokenStream {
    crate::select::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Builds an ad-hoc named-field projection, providing an ergonomic alias for [`select!`].
///
/// Eliminates syntactic stuttering when passing projections to query methods:
///
/// ```ignore
/// let query = select_from::<Postgres, User>()
///     .select(cols!(UserId, UserEmail as contact_email));
/// ```
#[proc_macro]
pub fn cols(input: TokenStream) -> TokenStream {
    select(input)
}

/// Declares a reusable named projection struct for query results.
///
/// Field types are read from the schema markers through
/// `<Column as Column>::RustType`, so a DTO never repeats column types and stays
/// in sync with the schema. Field names follow the column markers (`UserId`
/// becomes `user_id`) unless renamed with `as alias`. The generated struct
/// implements `FromRow`, so it decodes rows straight from the ordered result
/// values and is usable as a function return type:
///
/// ```ignore
/// dto! {
///     /// A user as returned by the repository.
///     pub struct UserSummary {
///         UserId,
///         UserEmail as contact_email,
///     }
/// }
/// ```
///
/// Field names follow the same rules as [`select!`]: `UserEmail as
/// contact_email` renames the field, and a trailing alias spelled as a type is
/// refused there as well, because that spelling is how a Rust cast is written.
#[proc_macro]
pub fn dto(input: TokenStream) -> TokenStream {
    crate::dto::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Builds a raw SQL expression with bound parameters and compile-time placeholder validation.
///
/// Parameters are bound safely as [`SqlVal`](::rurqe_core::expr::SqlVal) literals and rendered
/// using dialect-appropriate parameter placeholders (e.g. `$1` for PostgreSQL, `?` for SQLite),
/// preventing SQL injection vulnerabilities.
///
/// ```ignore
/// let pred = sql!("strftime('%Y-%m', date) = ?", "2024-01");
/// let explicit = sql!(bool, "data @> '{\"active\": true}'::jsonb");
/// let count = sql!(i64, "COUNT(*)");
/// ```
#[proc_macro]
pub fn sql(input: TokenStream) -> TokenStream {
    crate::raw_sql::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Builds a raw SQL expression, providing an ergonomic alias for [`sql!`].
#[proc_macro]
pub fn raw_sql(input: TokenStream) -> TokenStream {
    sql(input)
}

/// Derives SQL parameter and row-value support for a domain newtype.
///
/// The derive mirrors the declarative `impl_sql_binary_newtype!` and
/// `impl_sql_text_newtype!` helpers and adds a delegation mode, so a domain type
/// binds like a built-in scalar without repeating the conversion code by hand:
///
/// ```ignore
/// #[derive(SqlType)]
/// #[sql(transparent)]
/// struct UserId(i64);
///
/// #[derive(SqlType)]
/// #[sql(as = Bytes, len = 4)]
/// struct Txid([u8; 4]);
///
/// #[derive(SqlType)]
/// #[sql(as = Text)]
/// struct Address(String);
/// ```
///
/// The generated code implements `ToSqlVal`, `From<Self> for SqlVal`,
/// `FromSqlValue`, and the expression-compatibility impls, so the newtype is
/// usable both as a bound parameter and as a decoded row value, by value and by
/// reference.
///
/// The borrowed form of the newtype reads the wrapped value through a clone, so
/// a `#[sql(transparent)]` newtype requires its wrapped value to implement
/// `Clone`.
#[proc_macro_derive(SqlType, attributes(sql))]
pub fn sql_type(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    crate::sql_type::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Bakes the lifecycle artifacts a project configuration declares into the binary.
///
/// # Resolution
///
/// The single optional argument names a `rurqe.toml` resolved against the manifest
/// directory of the calling crate. The filesystem decides what the path is: a file
/// is the configuration, while a directory and a path naming nothing are rejected
/// with an error naming the remedy.
///
/// Without an argument the macro starts at the manifest directory, walks its
/// ancestors for a `rurqe.toml`, and falls back to the conventional
/// `<manifest>/rurqe` artifact layout when no ancestor declares one.
///
/// # What is baked
///
/// Only raw text: the `(file name, SQL text)` pairs of the top-level `*.sql`
/// scripts of the migration directory, the text of the `checkpoint.toml` marker
/// guarding that tail, the `(file name, DDL text)` pairs of the declarative stage,
/// and the text of the `schema.lock` snapshot. An absent artifact leaves the
/// lifecycle step it feeds switched off, while a `checkpoint-stage/` snapshot
/// beside the marker is an artifact of the toolchain and never becomes a script.
///
/// Nothing is parsed while the macro expands: the collection is assembled through
/// `::rurqe::migrate::EmbeddedMigrations::from_artifacts` at startup, which parses
/// the scripts, the marker, the stage, and the snapshot with the parsers of the
/// migration crate, so a malformed artifact stops the process instead of leaving a
/// gap in the recorded history.
///
/// Every file reaches the collection through `include_str!` over its absolute
/// path, so it is an input of the compilation: editing an artifact recompiles the
/// binary that carries it.
///
/// # Rejections
///
/// A directory argument, a path naming nothing, a configuration the strict parser
/// rejects, and a configuration holding neither a tail script nor a stage script
/// stop the compilation with an error naming the path and the remedy.
///
/// ```ignore
/// let embedded = embed_migrations!()?;
/// let embedded = embed_migrations!("../rurqe.toml")?;
/// ```
#[proc_macro]
pub fn embed_migrations(input: TokenStream) -> TokenStream {
    crate::embed_migrations::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}