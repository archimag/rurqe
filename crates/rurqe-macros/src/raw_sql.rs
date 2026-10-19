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

fn count_placeholders(template: &str) -> (usize, usize) {
    let mut q_count = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
            } else {
                q_count += 1;
            }
        }
    }

    let mut brace_count = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            brace_count += 1;
        }
    }

    (q_count, brace_count)
}

/// Expands parsed `sql!` / `raw_sql!` macro inputs into an `Expr<T>` construction block.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let parsed: RawSqlInput = syn::parse2(input)?;
    let template_val = parsed.template.value();
    let args_len = parsed.args.len();

    if args_len > 0 {
        let (q_count, brace_count) = count_placeholders(&template_val);
        if q_count != args_len && brace_count != args_len {
            let msg = if q_count > 0 {
                format!(
                    "raw SQL template expects {q_count} parameter(s), but {args_len} were provided"
                )
            } else if brace_count > 0 {
                format!(
                    "raw SQL template expects {brace_count} parameter(s), but {args_len} were provided"
                )
            } else {
                format!("raw SQL template expects 0 parameters, but {args_len} were provided")
            };
            return Err(syn::Error::new_spanned(&parsed.template, msg));
        }
    }

    let template = &parsed.template;
    let args = &parsed.args;

    let ty = match parsed.explicit_type {
        Some(ty) => quote! { #ty },
        None => quote! { _ },
    };

    Ok(quote! {
        {
            let __rurqe_params: ::std::vec::Vec<::rurqe_core::expr::SqlVal> = ::std::vec![
                #( ::core::convert::Into::<::rurqe_core::expr::SqlVal>::into(#args) ),*
            ];
            ::rurqe_core::expr::Expr::<#ty>::raw(#template, __rurqe_params)
        }
    })
}
