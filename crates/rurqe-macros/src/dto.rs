//! Expansion of the `dto!` reusable projection macro.
//!
//! The macro declares a named struct whose field types are read from the schema
//! markers through `<Column as Column>::RustType`, and implements
//! [`FromRow`](rurqe_core::row::FromRow) for it. A DTO can therefore be returned
//! from functions without repeating column types and without decoding rows by
//! tuple index.
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

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Ident, Token, Visibility};

use crate::parse::ProjectionList;

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

        let content;
        syn::braced!(content in input);
        let fields: ProjectionList = content.parse()?;

        Ok(Self {
            attrs,
            vis,
            ident,
            fields,
        })
    }
}

/// Expands a parsed `dto!` input into the struct declaration, its `FromRow` impl,
/// and the nested-row contracts of the same declaration.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let Dto {
        attrs,
        vis,
        ident,
        fields,
    } = syn::parse2(input)?;

    crate::parse::ensure_unique_field_names(&fields)?;

    let mut field_defs = TokenStream::new();
    let mut field_reads = TokenStream::new();
    let mut document_writes = TokenStream::new();
    let mut document_reads = TokenStream::new();
    let mut field_keys = Vec::new();
    let arity = Literal::usize_unsuffixed(fields.items.len());

    for (index, item) in fields.items.iter().enumerate() {
        let field_index = Literal::usize_unsuffixed(index);
        let field_attrs = &item.attrs;
        let field = item.field_ident()?;
        let Some(column) = item.column_path() else {
            return Err(syn::Error::new_spanned(
                item.target.tokens(),
                "`dto!` fields must be schema column markers; \
                 use `select!` for computed projections",
            ));
        };

        let key = Literal::string(&field_key(&field));
        field_keys.push(key.clone());

        field_defs.extend(quote! {
            #(#field_attrs)*
            #vis #field: <#column as ::rurqe::__core::schema::Column>::RustType,
        });
        field_reads.extend(quote! {
            #field: ::rurqe::__core::row::FromSqlValue::from_sql_value(
                values.get(#field_index).ok_or(
                    ::rurqe::__core::row::RowDecodeError::ArityMismatch {
                        expected: #arity,
                        actual: values.len(),
                    },
                )?,
            )?,
        });
        document_writes.extend(quote! {
            (#key, ::rurqe::__core::expr::ToSqlVal::to_sql_val(self.#field)),
        });
        document_reads.extend(quote! {
            #field: ::rurqe::__core::nested::nested_field(&document, #key)?,
        });
    }

    Ok(quote! {
        #(#attrs)*
        #[derive(Debug, Clone, PartialEq)]
        #vis struct #ident {
            #field_defs
        }

        impl ::rurqe::__core::row::FromRow for #ident {
            fn try_from_row(
                values: &[::rurqe::__core::expr::SqlVal],
            ) -> ::core::result::Result<Self, ::rurqe::__core::row::RowDecodeError> {
                if values.len() != #arity {
                    return ::core::result::Result::Err(
                        ::rurqe::__core::row::RowDecodeError::ArityMismatch {
                            expected: #arity,
                            actual: values.len(),
                        },
                    );
                }
                ::core::result::Result::Ok(#ident {
                    #field_reads
                })
            }
        }

        impl ::rurqe::__core::nested::MultisetRow for #ident {
            const FIELDS: &'static [&'static str] = &[#(#field_keys),*];
        }

        impl ::rurqe::__core::expr::ToSqlVal for #ident {
            fn to_sql_val(self) -> ::rurqe::__core::expr::SqlVal {
                ::rurqe::__core::nested::multiset_object(&[#document_writes])
            }
        }

        impl ::rurqe::__core::row::FromSqlValue for #ident {
            fn from_sql_value(
                value: &::rurqe::__core::expr::SqlVal,
            ) -> ::core::result::Result<Self, ::rurqe::__core::row::RowDecodeError> {
                let document = ::rurqe::__core::nested::json_object(value)?;
                ::core::result::Result::Ok(#ident {
                    #document_reads
                })
            }
        }
        impl ::rurqe::__core::expr::SqlArrayElement for #ident {}
    })
}

/// Derives the JSON key of a projected field.
///
/// The key follows the field name, and a field named after a Rust keyword keeps
/// the bare keyword: a field declared for the `Type` column is named `r#type` in
/// Rust and written as `type` in the document.
fn field_key(field: &Ident) -> String {
    let name = field.to_string();
    match name.strip_prefix("r#") {
        Some(stripped) => stripped.to_owned(),
        None => name,
    }
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
}
