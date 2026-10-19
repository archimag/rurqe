//! Expansion logic for the `sql!` and `raw_sql!` procedural macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitStr, Token, Type};

/// Parsed representation of `sql!(...)` and `raw_sql!(...)`.
pub struct RawSqlInput {
    pub explicit_type: Option<Type>,
    pub template: LitStr,
    pub args: Vec<Expr>,
}

impl Parse for RawSqlInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let explicit_type = if input.peek(Token![<]) {
            input.parse::<Token![<]>()?;
            let ty: Type = input.parse()?;
            input.parse::<Token![>]>()?;
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            Some(ty)
        } else if !input.peek(LitStr) {
            let ty: Type = input.parse()?;
            input.parse::<Token![,]>()?;
            Some(ty)
        } else {
            None
        };

        let template: LitStr = input.parse()?;

        let mut args = Vec::new();
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let arg: Expr = input.parse()?;
            args.push(arg);
        }

        Ok(Self {
            explicit_type,
            template,
            args,
        })
    }
}

/// Expands parsed `sql!` / `raw_sql!` macro inputs into an `Expr<T>` construction block.
///
/// The placeholder census and the split into literal text fragments are delegated
/// to `rurqe_core::expr::parse_raw_sql`, the same routine the runtime uses, so the
/// macro and the query engine can never disagree about what a template means. The
/// split happens once, at compile time; the generated code only binds the
/// parameters and hands the ready-made fragments to `Expr::raw_fragments`.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let parsed: RawSqlInput = syn::parse2(input)?;
    let template = parsed.template.value();
    let args_len = parsed.args.len();

    let fragments = ::rurqe_core::expr::parse_raw_sql(&template, args_len)
        .map_err(|error| syn::Error::new_spanned(&parsed.template, error.to_string()))?;
    let fragments: Vec<LitStr> = fragments
        .iter()
        .map(|fragment| LitStr::new(fragment, parsed.template.span()))
        .collect();

    let ty = match &parsed.explicit_type {
        Some(ty) => quote! { #ty },
        None => quote! { _ },
    };
    let args = &parsed.args;

    Ok(quote! {
        {
            let __rurqe_params: ::std::vec::Vec<::rurqe_core::expr::SqlVal> = ::std::vec![
                #( ::core::convert::Into::<::rurqe_core::expr::SqlVal>::into(#args) ),*
            ];
            ::rurqe_core::expr::Expr::<#ty>::raw_fragments(
                ::std::vec![ #( ::std::string::String::from(#fragments) ),* ],
                __rurqe_params,
            )
        }
    })
}
