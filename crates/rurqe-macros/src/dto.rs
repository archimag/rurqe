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
            #vis #field: <#column as ::rurqe_core::schema::Column>::RustType,
        });
        field_reads.extend(quote! {
            #field: ::rurqe_core::row::FromSqlValue::from_sql_value(
                values.get(#field_index).ok_or(
                    ::rurqe_core::row::RowDecodeError::ArityMismatch {
                        expected: #arity,
                        actual: values.len(),
                    },
                )?,
            )?,
        });
        document_writes.extend(quote! {
            (#key, ::rurqe_core::expr::ToSqlVal::to_sql_val(self.#field)),
        });
        document_reads.extend(quote! {
            #field: ::rurqe_core::nested::nested_field(&document, #key)?,
        });
    }

    Ok(quote! {
        #(#attrs)*
        #[derive(Debug, Clone, PartialEq)]
        #vis struct #ident {
            #field_defs
        }

        impl ::rurqe_core::row::FromRow for #ident {
            fn try_from_row(
                values: &[::rurqe_core::expr::SqlVal],
            ) -> ::core::result::Result<Self, ::rurqe_core::row::RowDecodeError> {
                if values.len() != #arity {
                    return ::core::result::Result::Err(
                        ::rurqe_core::row::RowDecodeError::ArityMismatch {
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

        impl ::rurqe_core::nested::MultisetRow for #ident {
            const FIELDS: &'static [&'static str] = &[#(#field_keys),*];
        }

        impl ::rurqe_core::expr::ToSqlVal for #ident {
            fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                ::rurqe_core::nested::multiset_object(&[#document_writes])
            }
        }

        impl ::rurqe_core::row::FromSqlValue for #ident {
            fn from_sql_value(
                value: &::rurqe_core::expr::SqlVal,
            ) -> ::core::result::Result<Self, ::rurqe_core::row::RowDecodeError> {
                let document = ::rurqe_core::nested::json_object(value)?;
                ::core::result::Result::Ok(#ident {
                    #document_reads
                })
            }
        }

        impl ::rurqe_core::expr::SqlArrayElement for #ident {}
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
