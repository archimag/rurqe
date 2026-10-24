//! Procedural macros for RURQE query builder and relational execution engine.

mod dto;
mod parse;
mod raw_sql;
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
/// usable both as a bound parameter and as a decoded row value.
#[proc_macro_derive(SqlType, attributes(sql))]
pub fn sql_type(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    crate::sql_type::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
