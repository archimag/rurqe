//! Procedural macros for RURQE query builder and relational execution engine.

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
