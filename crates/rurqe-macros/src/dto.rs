//! Expansion of the `dto!` reusable projection macro.
//!
//! The macro declares a named struct whose field types are read from the schema
//! markers through `<Column as Column>::RustType`, and implements [`FromRow`] for
//! it. A DTO can therefore be returned from functions without repeating column
//! types and without decoding rows by tuple index.
//!
//! [`FromRow`]: rurqe_core::row::FromRow

use proc_macro2::TokenStream;
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

/// Expands a parsed `dto!` input into the struct declaration and its `FromRow` impl.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let Dto {
        attrs,
        vis,
        ident,
        fields,
    } = syn::parse2(input)?;

    let mut field_defs = TokenStream::new();
    let mut field_reads = TokenStream::new();

    for item in &fields.items {
        let field_attrs = &item.attrs;
        let field = item.field_ident();
        let column = &item.column;

        field_defs.extend(quote! {
            #(#field_attrs)*
            #vis #field: <#column as ::rurqe_core::schema::Column>::RustType,
        });
        field_reads.extend(quote! {
            #field: ::rurqe_core::row::FromSqlValue::from_sql_value(values.next()?)?,
        });
    }

    Ok(quote! {
        #(#attrs)*
        #[derive(Debug, Clone, PartialEq)]
        #vis struct #ident {
            #field_defs
        }

        impl ::rurqe_core::row::FromRow for #ident {
            fn from_row(values: &[::rurqe_core::expr::SqlVal]) -> ::core::option::Option<Self> {
                let mut values = values.iter();
                let dto = #ident {
                    #field_reads
                };
                values.next().is_none().then_some(dto)
            }
        }
    })
}
