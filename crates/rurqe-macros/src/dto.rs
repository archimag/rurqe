//! Expansion of the `dto!` reusable projection macro.
//!
//! The macro declares a named struct whose field types are read from the schema
//! markers through `<Column as Column>::RustType`, and implements
//! [`FromRow`](rurqe_core::row::FromRow) for it. A DTO can therefore be returned
//! from functions without repeating column types and without decoding rows by
//! tuple index.
//!
//! The declaration is concrete: every field type is read from the schema column
//! marker it names, so the expansion has no type parameter to give the struct it
//! emits. A declaration carrying generic parameters or a `where` clause is
//! rejected with a diagnostic naming `dto!` and the concrete form it expects,
//! instead of the parser's generic missing-body error.
//!
//! The same declaration describes the row of a related collection: the struct
//! implements [`MultisetRow`](rurqe_core::nested::MultisetRow) together with
//! [`ToSqlVal`](rurqe_core::expr::ToSqlVal),
//! [`FromSqlValue`](rurqe_core::row::FromSqlValue), and
//! [`SqlArrayElement`](rurqe_core::expr::SqlArrayElement), so a DTO is read as
//! `Vec<Dto>` from a nested projection and binds back as the JSON document of its
//! fields. The keys of that document are the declared field names in declaration
//! order, which is what `MultisetRow::FIELDS` reports.
//!
//! Every path the expansion emits is anchored at `::rurqe::__core`, the re-export
//! the facade publishes for its macros, so a crate that depends only on `rurqe`
//! compiles the declared struct without adding `rurqe-core` to its manifest.
//!
//! A DTO is a decoding and nested-row type, not a projection: a statement
//! projects its rows with `select!`, and the DTO reads the row the resulting
//! projection renders. Writing a `select!` projection that names the same
//! fields keeps its row type aligned with the declaration, so a row decodes
//! into the DTO without an intermediate tuple.
//!
//! The row and nested-row contracts are rendered by [`crate::record`], the same
//! emitter `select!` uses, so a `dto!` field list and a matching `select!`
//! projection list decode identically and publish identical JSON keys.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Ident, Token, Visibility};

use crate::parse::ProjectionList;
use crate::record::{Record, RecordField};

/// A parsed `dto!` declaration.
struct Dto {
    attrs: Vec<Attribute>,
    vis: Visibility,
    ident: Ident,
    fields: ProjectionList,
}

impl Parse for Dto {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis: Visibility = input.parse()?;
        input.parse::<Token![struct]>()?;
        let ident: Ident = input.parse()?;
        reject_generics(input)?;

        let content;
        syn::braced!(content in input);
        // `ProjectionList::parse` verifies that it consumes the whole struct
        // body, so a declaration written after a parenthesized or braced list is
        // reported there rather than dropped silently here.
        let fields: ProjectionList = content.parse()?;

        Ok(Self {
            attrs,
            vis,
            ident,
            fields,
        })
    }
}

/// Rejects a declaration carrying generic parameters or a `where` clause.
///
/// Every field type is read from the schema column marker the field names, so the
/// expansion has no type parameter to give the struct it emits and no bound to
/// state for one. The struct body is what the parser looks for next, so a
/// declaration carrying generics would fail there with an error naming neither
/// `dto!` nor the form it expects; the input is refused here, where both can be
/// named.
fn reject_generics(input: ParseStream) -> syn::Result<()> {
    if input.peek(Token![<]) {
        return Err(input.error(
            "`dto!` cannot declare generic parameters; \
             declare a concrete struct whose fields are schema column markers",
        ));
    }

    if input.peek(Token![where]) {
        return Err(input.error(
            "`dto!` cannot declare a `where` clause; \
             declare a concrete struct whose fields are schema column markers",
        ));
    }

    Ok(())
}

/// Expands a parsed `dto!` input into the struct declaration and the record
/// contract of its fields.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let Dto {
        attrs,
        vis,
        ident,
        fields,
    } = syn::parse2(input)?;

    crate::parse::ensure_unique_field_names(&fields)?;

    let mut record_fields = Vec::with_capacity(fields.len());
    for item in &fields.items {
        let field = item.field_ident()?;
        let Some(column) = item.column_path() else {
            return Err(syn::Error::new_spanned(
                item.target.tokens(),
                "`dto!` fields must be schema column markers; \
                 use `select!` for computed projections",
            ));
        };
        let field_attrs = &item.attrs;

        record_fields.push(RecordField {
            attrs: quote! { #(#field_attrs)* },
            vis: vis.clone(),
            ident: field,
            ty: quote! { <#column as ::rurqe::__core::schema::Column>::RustType },
        });
    }

    let declaration = Record {
        attrs: quote! { #(#attrs)* },
        vis,
        ident,
        params: Vec::new(),
        fields: record_fields,
    };

    Ok(declaration.expand())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn keyword_markers_without_a_raw_form_are_rejected() {
        let error = expand(quote! {
            pub struct Bad {
                Crate,
            }
        })
        .expect_err("the marker cannot name a field");

        assert!(error.to_string().contains("raw-identifier form"));
    }

    #[test]
    fn keyword_markers_accept_an_explicit_alias() {
        let expanded = expand(quote! {
            pub struct Good {
                crate_id: Crate,
            }
        })
        .expect("the alias names the field");

        assert!(expanded.to_string().contains("struct Good"));
    }

    #[test]
    fn a_grouped_projection_list_declares_the_fields() {
        let expanded = expand(quote! {
            pub struct Good {
                (UserId, UserName)
            }
        })
        .expect("the group carries the projections");
        let rendered = expanded.to_string();

        assert!(rendered.contains("user_id"), "missing field: {rendered}");
        assert!(rendered.contains("user_name"), "missing field: {rendered}");

        let expanded = expand(quote! {
            pub struct Braced {
                { UserId, UserName }
            }
        })
        .expect("the braces carry the projections");
        let rendered = expanded.to_string();

        assert!(rendered.contains("user_id"), "missing field: {rendered}");
        assert!(rendered.contains("user_name"), "missing field: {rendered}");
    }

    #[test]
    fn tokens_after_a_grouped_projection_list_are_rejected() {
        for source in [
            quote! {
                pub struct Bad {
                    (UserId, UserName),
                    extra: i64,
                }
            },
            quote! {
                pub struct Bad {
                    { UserId },
                    extra: i64,
                }
            },
        ] {
            let error = expand(source).expect_err("the trailing declaration belongs to no field");

            assert!(
                error.to_string().contains("unexpected tokens after the projection list"),
                "unexpected message: {error}"
            );
        }
    }

    #[test]
    fn a_trailing_comma_after_a_grouped_projection_list_is_tolerated() {
        let expanded = expand(quote! {
            pub struct Good {
                (UserId, UserName),
            }
        })
        .expect("a trailing comma ends the list");
        let rendered = expanded.to_string();

        assert!(rendered.contains("user_id"), "missing field: {rendered}");
        assert!(rendered.contains("user_name"), "missing field: {rendered}");
    }

    #[test]
    fn duplicate_field_names_are_rejected() {
        let error = expand(quote! {
            pub struct Bad {
                id: UserId,
                id: UserName,
            }
        })
        .expect_err("two fields cannot share one name");

        assert_eq!(
            error.to_string(),
            "projection field `id` is produced by more than one projection; \
             rename one of them with `as <alias>` or `alias: <expr>`"
        );
    }

    #[test]
    fn fields_reducing_to_the_same_name_are_rejected() {
        let error = expand(quote! {
            pub struct Bad {
                UserId as id,
                UserName as id,
            }
        })
        .expect_err("two aliases cannot share one name");

        assert!(error.to_string().contains("projection field `id`"));
        assert!(error.to_string().contains("rename one of them"));
    }

    #[test]
    fn expansions_are_anchored_at_the_facade() {
        let expanded = expand(quote! {
            pub struct User {
                UserId,
            }
        })
        .expect("the declaration expands");
        let rendered = expanded.to_string();

        assert!(rendered.contains(":: rurqe :: __core"));
    }

    #[test]
    fn the_declaration_emits_no_projection_surface() {
        let expanded = expand(quote! {
            pub struct User {
                UserId,
            }
        })
        .expect("the declaration expands");
        let rendered = expanded.to_string();

        // A DTO decodes rows and describes the row of a nested collection; a
        // statement projects its rows through `select!`. The expansion carries
        // no `Selection` impl — surface no builder consumes — so the dead
        // projection arms cannot silently return.
        assert!(!rendered.contains("Selection"));
        assert!(!rendered.contains("push_projections"));
        assert!(!rendered.contains("push_column_names"));
    }
    #[test]
    fn a_generic_declaration_names_the_macro_and_the_expected_form() {
        let error = expand(quote! {
            pub struct Wrapper<T> {
                UserId,
            }
        })
        .expect_err("a generic declaration is refused");

        assert_eq!(
            error.to_string(),
            "`dto!` cannot declare generic parameters; \
             declare a concrete struct whose fields are schema column markers"
        );
    }

    #[test]
    fn a_where_clause_names_the_macro_and_the_expected_form() {
        let error = expand(quote! {
            pub struct Wrapper
            where
                UserId: Copy,
            {
                UserId,
            }
        })
        .expect_err("a where clause is refused");

        assert_eq!(
            error.to_string(),
            "`dto!` cannot declare a `where` clause; \
             declare a concrete struct whose fields are schema column markers"
        );
    }
}
