//! Procedural macros for RURQE query builder and relational execution engine.

mod dto;
#[allow(dead_code)]
mod parse;
mod select;

use proc_macro::TokenStream;

/// Builds an ad-hoc named-field projection for `SELECT` queries.
///
/// The macro expands into an anonymous record whose fields are named after the
/// projected columns (or their `as alias`), typed through
/// `<Column as Column>::RustType`, and implements both `Selection` and
/// `FromRow` for it. The expansion evaluates to a zero-sized selection marker
/// that can be passed straight to a query builder, while the produced row is
/// exposed through inference so callers never index by tuple position:
///
/// ```ignore
/// let query = SelectQuery::<Postgres>::new()
///     .from::<User>()
///     .select(select!(UserId, UserEmail as contact_email));
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
#[proc_macro]
pub fn dto(input: TokenStream) -> TokenStream {
    crate::dto::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
